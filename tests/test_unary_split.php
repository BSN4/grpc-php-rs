<?php
/**
 * Split-batch unary test — the pattern every generated PHP client uses:
 * `UnaryCall::start()` issues a send-only startBatch, `wait()` issues the
 * recv startBatch afterwards.
 *
 * Asserts that results match the single-batch form: message, initial metadata,
 * trailing metadata, status codes, deadline and cancellation.
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_unary_split.php
 */
declare(strict_types=1);

echo "=== Split-batch Unary Test ===\n\n";

/** Matches SLOW_ECHO_DELAY in tests/server/src/main.rs. */
const SLOW_ECHO_DELAY = 0.250;

$tests = 0;
$passed = 0;

function check(string $name, bool $result, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

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

/** Issues the send batch only, exactly as UnaryCall::start() does. */
function start_unary(Grpc\Channel $channel, string $method, string $body, ?Grpc\Timeval $deadline = null): Grpc\Call {
    $call = new Grpc\Call($channel, $method, $deadline ?? Grpc\Timeval::infFuture());
    $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [],
        Grpc\OP_SEND_MESSAGE => ['message' => encode_payload($body)],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    ]);
    return $call;
}

/** Issues the recv batch only, exactly as UnaryCall::wait() does. */
function wait_unary(Grpc\Call $call): object {
    return $call->startBatch([
        Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true,
        Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
}

$target = 'localhost:50051';
$channel = new Grpc\Channel($target, ['credentials' => Grpc\ChannelCredentials::createInsecure()]);

// -----------------------------------------------------------------------------
// Case 1: success — message plus binary initial metadata.
// -----------------------------------------------------------------------------
echo "--- Case 1: success response ---\n";

$e = wait_unary(start_unary($channel, '/grpc.testing.TestService/Echo', 'hi'));
check('status OK', ($e->status->code ?? -1) === 0, 'code=' . ($e->status->code ?? -1));
check('message echoed', ($e->message ?? '') === encode_payload('hi'));
check(
    'binary initial metadata intact',
    ($e->metadata['x-test-binary-bin'][0] ?? null) === "hello\x00\xfftrailer"
);

// -----------------------------------------------------------------------------
// Case 2: error status — code, details and binary trailing metadata.
// -----------------------------------------------------------------------------
echo "\n--- Case 2: error response ---\n";

$e = wait_unary(start_unary($channel, '/grpc.testing.TestService/ErrorResponse', 'x'));
check('status INTERNAL (13)', ($e->status->code ?? -1) === 13, 'code=' . ($e->status->code ?? -1));
check('status details preserved', ($e->status->details ?? '') === 'test error');
check(
    'rich-status trailer intact',
    ($e->status->metadata['grpc-status-details-bin'][0] ?? null) === "hello\x00\xfftrailer"
);
check('message is null on error', property_exists($e, 'message') && $e->message === null);

// -----------------------------------------------------------------------------
// Case 3: empty response body — must be "" like the C extension, not null.
// -----------------------------------------------------------------------------
echo "\n--- Case 3: empty response ---\n";

$e = wait_unary(start_unary($channel, '/grpc.testing.TestService/EmptyResponse', 'x'));
check('status OK', ($e->status->code ?? -1) === 0, 'code=' . ($e->status->code ?? -1));
check('message is empty string, not null', ($e->message ?? null) === '');

// -----------------------------------------------------------------------------
// Case 4: 64KB response body.
// -----------------------------------------------------------------------------
echo "\n--- Case 4: large response ---\n";

$e = wait_unary(start_unary($channel, '/grpc.testing.TestService/LargeResponse', 'x'));
check('status OK', ($e->status->code ?? -1) === 0, 'code=' . ($e->status->code ?? -1));
check('64KB body received', strlen($e->message ?? '') > 65536, strlen($e->message ?? '') . ' bytes');

// -----------------------------------------------------------------------------
// Case 5: unknown method — status must surface on the recv batch.
// -----------------------------------------------------------------------------
echo "\n--- Case 5: unknown method ---\n";

$e = wait_unary(start_unary($channel, '/grpc.testing.TestService/NoSuchMethod', 'x'));
check('status UNIMPLEMENTED (12)', ($e->status->code ?? -1) === 12, 'code=' . ($e->status->code ?? -1));

// -----------------------------------------------------------------------------
// Case 6: deadline — set on the send batch, enforced against the slow method.
// -----------------------------------------------------------------------------
echo "\n--- Case 6: deadline ---\n";

$deadline = Grpc\Timeval::now()->add(new Grpc\Timeval(50 * 1000)); // 50ms < SLOW_ECHO_DELAY
$e = wait_unary(start_unary($channel, '/grpc.testing.TestService/SlowEcho', 'hi', $deadline));
check('status DEADLINE_EXCEEDED (4)', ($e->status->code ?? -1) === 4, 'code=' . ($e->status->code ?? -1));

// -----------------------------------------------------------------------------
// Case 7: cancellation between start and wait.
// -----------------------------------------------------------------------------
echo "\n--- Case 7: cancel before wait ---\n";

$call = start_unary($channel, '/grpc.testing.TestService/Echo', 'hi');
$call->cancel();
try {
    $e = wait_unary($call);
    check('cancelled call is not OK', ($e->status->code ?? -1) !== 0, 'code=' . ($e->status->code ?? -1));
} catch (Throwable $t) {
    // Divergence from ext-grpc (which returns status code 1) is tracked in
    // the parity ledger; here require the failure to actually be the cancel,
    // not an arbitrary Throwable.
    check('cancelled call is not OK', stripos($t->getMessage(), 'cancel') !== false,
        get_class($t) . ': ' . $t->getMessage());
}

// -----------------------------------------------------------------------------
echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
