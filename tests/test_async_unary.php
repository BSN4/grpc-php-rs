<?php
/**
 * Async (split start/wait) unary calls must run concurrently — issue #18.
 *
 * google/gax's promise-based flow calls UnaryCall::start() (send-only batch)
 * for N calls up front, then waits each promise (recv batch). The requests
 * must all be in flight after start(): N calls against a server that delays
 * each response by 200ms must complete in ~200ms total, not N x 200ms.
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_async_unary.php
 */
declare(strict_types=1);

echo "=== Async Unary Concurrency Test ===\n\n";

function encode_varint(int $value): string {
    $buf = '';
    while ($value > 0x7f) {
        $buf .= chr(($value & 0x7f) | 0x80);
        $value >>= 7;
    }
    return $buf . chr($value & 0x7f);
}

function encode_payload(string $data): string {
    if ($data === '') return '';
    return "\x0a" . encode_varint(strlen($data)) . $data;
}

function decode_body(?string $wire): ?string {
    if ($wire === null || $wire === '') return $wire;
    $len = 0; $shift = 0; $i = 1;
    while (true) {
        $b = ord($wire[$i]);
        $len |= ($b & 0x7f) << $shift;
        $i++;
        if (!($b & 0x80)) break;
        $shift += 7;
    }
    return substr($wire, $i, $len);
}

$tests = 0;
$passed = 0;
function check(string $name, bool $result, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

$channel = new Grpc\Channel('localhost:50051', []);

// -----------------------------------------------------------------------------
// Case 1: N split start()/wait() unary calls overlap their round trips
// -----------------------------------------------------------------------------
echo "--- Case 1: concurrent split unary calls ---\n";

$n = 10;
$t0 = hrtime(true);

$calls = [];
for ($i = 0; $i < $n; $i++) {
    $call = new Grpc\Call($channel, '/grpc.testing.TestService/Echo', Grpc\Timeval::infFuture());
    // UnaryCall::start() shape: send-only batch, half-close included
    $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [],
        Grpc\OP_SEND_MESSAGE => ['message' => encode_payload('sleep:200')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    ]);
    $calls[] = $call;
}

$ok = 0;
$bodies_ok = 0;
foreach ($calls as $call) {
    // UnaryCall::wait() shape
    $r = $call->startBatch([
        Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true,
        Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    if ($r->status->code === 0) $ok++;
    if ($r->message !== null && decode_body($r->message) === 'sleep:200') $bodies_ok++;
}
$elapsed_ms = (hrtime(true) - $t0) / 1e6;

check("all {$n} calls OK", $ok === $n, "ok={$ok}");
check("all {$n} responses echoed intact", $bodies_ok === $n, "bodies_ok={$bodies_ok}");
// Serial execution would take >= n*200ms = 2000ms. Concurrent ~200-400ms.
check(
    sprintf("calls ran concurrently (%.0fms for %d x 200ms delay)", $elapsed_ms, $n),
    $elapsed_ms < 1200,
    sprintf("took %.0fms, serial would be %dms", $elapsed_ms, $n * 200)
);

// -----------------------------------------------------------------------------
// Case 2: split-start server streaming still works (same send-only batch shape)
// -----------------------------------------------------------------------------
echo "\n--- Case 2: split-start server streaming ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/StreamEcho', Grpc\Timeval::infFuture());
$call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
    Grpc\OP_SEND_MESSAGE => ['message' => encode_payload('repeat:3')],
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
]);
// grpc/grpc ServerStreamingCall: metadata fetched separately, then reads
$md = $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA => true]);
check('streaming: initial metadata received', is_array($md->metadata ?? null) || is_object($md->metadata ?? null));

$messages = [];
while (true) {
    $r = $call->startBatch([Grpc\OP_RECV_MESSAGE => true]);
    if ($r->message === null) break;
    $messages[] = decode_body($r->message);
}
$s = $call->startBatch([Grpc\OP_RECV_STATUS_ON_CLIENT => true]);

check('streaming: 3 messages received', count($messages) === 3, 'got ' . count($messages));
check('streaming: status OK', $s->status->code === 0, "code={$s->status->code}");

// -----------------------------------------------------------------------------
// Case 3: split unary error keeps rich status (details-bin, #17 regression)
// -----------------------------------------------------------------------------
echo "\n--- Case 3: split unary error surfaces rich status ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/ErrorResponse', Grpc\Timeval::infFuture());
$call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
    Grpc\OP_SEND_MESSAGE => ['message' => encode_payload('x')],
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
]);
$r = $call->startBatch([
    Grpc\OP_RECV_INITIAL_METADATA => true,
    Grpc\OP_RECV_MESSAGE => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT => true,
]);
check('error: message is null', $r->message === null);
check('error: status INTERNAL', $r->status->code === 13, "code={$r->status->code}");
$trailing = (array)($r->status->metadata ?? []);
check(
    'error: grpc-status-details-bin present',
    isset($trailing['grpc-status-details-bin'][0]) && $trailing['grpc-status-details-bin'][0] === "hello\x00\xfftrailer"
);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
