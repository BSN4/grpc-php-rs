<?php
/**
 * Connection lifecycle under the per-thread runtime model (src/runtime.rs):
 * nothing drives the runtime while PHP is outside a call, so connection
 * events that arrive while idle (server closed/restarted, GOAWAY) must be
 * processed BEFORE the next request is dispatched, not discovered mid-request.
 *
 * Regression: the first call after a restart-while-idle used to fail with
 * UNKNOWN "transport error" (not retried by gax); it must reconnect
 * transparently (code 0), as it did on the old shared runtime and as C-core does.
 *
 * Needs a controllable server in the same container (kill/restart).
 */
declare(strict_types=1);

echo "=== Connection lifecycle (idle, restart, outage) ===\n\n";

$tests = 0; $passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}
function ep(string $d): string { return "\x0a" . chr(strlen($d)) . $d; }
$sh = fn(string $c) => shell_exec($c . ' 2>&1');

$ch = new Grpc\Channel('127.0.0.1:50051', ['credentials' => Grpc\ChannelCredentials::createInsecure()]);
$call = function (int $dlMs = 3000) use ($ch): array {
    $c = new Grpc\Call($ch, '/grpc.testing.TestService/Echo', Grpc\Timeval::now()->add(new Grpc\Timeval($dlMs * 1000)));
    $t0 = hrtime(true);
    $r = $c->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true,
        Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    return [$r->status->code, $r->status->details, (hrtime(true)-$t0)/1e6];
};

[$code] = $call();
check('warm call OK', $code === 0, "code={$code}");

sleep(2);
[$code, , $ms] = $call();
check('call after 2s idle OK', $code === 0, "code={$code}");

// Restart the server while PHP is idle; the first call must reconnect transparently.
for ($round = 1; $round <= 3; $round++) {
    $sh('kill $(pidof grpc-test-server); sleep 0.3; (grpc-test-server >/dev/null 2>&1 &); sleep 1');
    [$code, $details, $ms] = $call();
    check("round {$round}: first call after restart-while-idle is OK (transparent reconnect)",
        $code === 0, sprintf('code=%d details="%s" %.1fms', $code, $details, $ms));
}

// Server down entirely: fail fast with UNAVAILABLE (retryable), never UNKNOWN.
$sh('kill $(pidof grpc-test-server); sleep 0.5');
[$code, , $ms] = $call();
check('server down: UNAVAILABLE (14), not UNKNOWN', $code === 14, "code={$code}");
check('server down: fails fast', $ms < 1000, sprintf('%.0fms', $ms));
$sh('(grpc-test-server >/dev/null 2>&1 &); sleep 1');
[$code] = $call();
check('server back: OK', $code === 0, "code={$code}");

// Outage mid-burst: only OK or UNAVAILABLE may appear (retryable), and recovery is transparent.
// Time-based burst (1.5 s) so the outage (kill at 0.3 s, restart at 0.7 s) is
// guaranteed to land inside it regardless of per-call speed.
$sh('(sleep 0.3; kill $(pidof grpc-test-server); sleep 0.4; grpc-test-server >/dev/null 2>&1 &) >/dev/null 2>&1 &');
$codes = [];
$end = microtime(true) + 1.5;
while (microtime(true) < $end) { [$c] = $call(500); $codes[$c] = ($codes[$c] ?? 0) + 1; }
$unexpected = array_diff_key($codes, [0 => 1, 14 => 1]);
check('mid-burst outage: only OK/UNAVAILABLE statuses', $unexpected === [], 'codes=' . json_encode($codes));
check('mid-burst outage: outage was actually exercised (some UNAVAILABLE)', ($codes[14] ?? 0) > 0, 'codes=' . json_encode($codes));
usleep(800_000);
[$code] = $call();
check('after outage: OK again', $code === 0, "code={$code}");

// Server streaming with a long pause mid-stream (runtime not driven during the pause).
$c = new Grpc\Call($ch, '/grpc.testing.TestService/StreamEcho', Grpc\Timeval::now()->add(new Grpc\Timeval(5_000_000)));
$c->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('repeat:300')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true, Grpc\OP_RECV_INITIAL_METADATA=>true]);
$n = 0;
while (true) { $r = $c->startBatch([Grpc\OP_RECV_MESSAGE=>true]); if ($r->message === null) break; $n++; if ($n === 150) usleep(500_000); }
$s = $c->startBatch([Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
check('stream with 500ms mid-stream pause: all messages + OK', $n === 300 && $s->status->code === 0, "n={$n} code={$s->status->code}");

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
