<?php
/**
 * The call deadline must bound the ENTIRE call, including connection
 * establishment. A fresh channel to a blackholed endpoint (TEST-NET-1,
 * 192.0.2.1 — unrouted everywhere, SYNs are silently dropped) with a 2 s
 * deadline must fail with DEADLINE_EXCEEDED at ~2 s, not hang for the
 * kernel's SYN-retry budget (~2 min). ext-grpc honors the deadline here
 * (measured: code 4 at 5.0 s with a 5 s deadline); before the fix we
 * returned UNAVAILABLE after 136 s.
 *
 * Self-validating: if the address ever answers (refused/unreachable), the
 * fast UNAVAILABLE fails the "blackhole" assertion instead of passing.
 */
declare(strict_types=1);

echo "=== Deadline covers connection establishment ===\n\n";

$tests = 0; $passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}
function ep(string $d): string { return "\x0a" . chr(strlen($d)) . $d; }

function blackholed_call(array $chanArgs, ?int $deadlineMs, string $method = '/grpc.testing.TestService/Echo'): array {
    $ch = new Grpc\Channel('192.0.2.1:50051', ['credentials' => Grpc\ChannelCredentials::createInsecure()] + $chanArgs);
    $deadline = $deadlineMs === null ? Grpc\Timeval::infFuture() : Grpc\Timeval::now()->add(new Grpc\Timeval($deadlineMs * 1000));
    $call = new Grpc\Call($ch, $method, $deadline);
    $t0 = hrtime(true);
    $r = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('hi')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    return [$r->status->code, (hrtime(true) - $t0) / 1e6, $r->status->details];
}

// 1. Single-batch unary, fresh channel, 2s deadline
[$code, $ms, $details] = blackholed_call(['grpc.probe' => 'a'], 2000);
check('blackhole is real (did not fail fast)', $ms > 1500, sprintf('returned in %.0fms code=%d', $ms, $code));
check('unary: DEADLINE_EXCEEDED (4)', $code === 4, "code={$code} details={$details}");
check('unary: failed at the deadline, not the SYN timeout', $ms < 4000, sprintf('%.0fms', $ms));

// 2. Split start()/wait() (gax pattern) — dispatch happens in start(), wait must still honor the deadline
$ch = new Grpc\Channel('192.0.2.1:50051', ['credentials' => Grpc\ChannelCredentials::createInsecure(), 'grpc.probe' => 'b']);
$call = new Grpc\Call($ch, '/grpc.testing.TestService/Echo', Grpc\Timeval::now()->add(new Grpc\Timeval(2_000_000)));
$t0 = hrtime(true);
$call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('hi')], Grpc\OP_SEND_CLOSE_FROM_CLIENT => true]);
$startMs = (hrtime(true) - $t0) / 1e6;
$r = $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA => true, Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true]);
$ms = (hrtime(true) - $t0) / 1e6;
check('split: start() does not block on connect', $startMs < 500, sprintf('%.0fms', $startMs));
check('split: DEADLINE_EXCEEDED (4)', $r->status->code === 4, "code={$r->status->code}");
check('split: failed at the deadline', $ms > 1500 && $ms < 4000, sprintf('%.0fms', $ms));

// 3. Server streaming shape (RECV_MESSAGE without RECV_STATUS first)
$ch = new Grpc\Channel('192.0.2.1:50051', ['credentials' => Grpc\ChannelCredentials::createInsecure(), 'grpc.probe' => 'c']);
$call = new Grpc\Call($ch, '/grpc.testing.TestService/StreamEcho', Grpc\Timeval::now()->add(new Grpc\Timeval(2_000_000)));
$t0 = hrtime(true);
$call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('x')], Grpc\OP_SEND_CLOSE_FROM_CLIENT => true]);
$m = $call->startBatch([Grpc\OP_RECV_MESSAGE => true]);
$s = $call->startBatch([Grpc\OP_RECV_STATUS_ON_CLIENT => true]);
$ms = (hrtime(true) - $t0) / 1e6;
check('stream: message null + DEADLINE_EXCEEDED', $m->message === null && $s->status->code === 4, "code={$s->status->code}");
check('stream: failed at the deadline', $ms > 1500 && $ms < 4000, sprintf('%.0fms', $ms));

// 4. Infinite deadline must not hang for minutes either: connect attempts are
//    bounded (C-core's minimum connect deadline is 20 s), then UNAVAILABLE.
[$code, $ms, ] = blackholed_call(['grpc.probe' => 'd'], null);
check('infinite deadline: fails as UNAVAILABLE (14)', $code === 14, "code={$code}");
check('infinite deadline: bounded by the connect timeout (~20s), not ~2min', $ms > 10000 && $ms < 40000, sprintf('%.0fms', $ms));

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
