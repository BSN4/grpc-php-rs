<?php
/**
 * Split-batch unary test — the pattern every generated PHP client uses:
 * `UnaryCall::start()` issues a send-only startBatch, `wait()` issues the
 * recv startBatch afterwards.
 *
 * Two things are asserted:
 *
 *   1. Results match the single-batch form (message, initial metadata,
 *      trailing metadata, status, deadline, cancellation).
 *   2. Calls started before any of them is awaited overlap on the wire.
 *      Buffering the request until the recv batch makes a batch of N calls
 *      cost N round trips instead of one, which is what
 *      https://github.com/BSN4/grpc-php-rs/issues/18 reported: 100 products
 *      through the Merchant API took 100-130s instead of 2-5s.
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
    check('cancelled call is not OK', true);
}

// -----------------------------------------------------------------------------
// Case 8: concurrency — the regression this file exists for.
//
// Ten calls against a method that sleeps SLOW_ECHO_DELAY server-side. If the
// requests go out during start(), the whole batch costs about one delay; if
// they are buffered until wait(), it costs ten. The threshold sits far from
// both so a loaded CI runner cannot flip it.
// -----------------------------------------------------------------------------
echo "\n--- Case 8: concurrent calls overlap ---\n";

$n = 10;
$serial = $n * SLOW_ECHO_DELAY;
$budget = $serial / 2;

$calls = [];
$t0 = microtime(true);
for ($i = 0; $i < $n; $i++) {
    $calls[] = start_unary($channel, '/grpc.testing.TestService/SlowEcho', "msg{$i}");
}
$startPhase = microtime(true) - $t0;

$correct = 0;
foreach ($calls as $i => $call) {
    $e = wait_unary($call);
    if (($e->status->code ?? -1) === 0 && ($e->message ?? '') === encode_payload("msg{$i}")) {
        $correct++;
    }
}
$total = microtime(true) - $t0;

printf("  %d calls, %.2fs total (serialised would be ~%.2fs)\n", $n, $total, $serial);
check("all {$n} responses correct and in order", $correct === $n, "{$correct}/{$n}");
check(
    sprintf('batch completed in under %.2fs', $budget),
    $total < $budget,
    sprintf('took %.2fs', $total)
);
check(
    'start() does not block on the response',
    $startPhase < SLOW_ECHO_DELAY,
    sprintf('start phase took %.2fs', $startPhase)
);

// -----------------------------------------------------------------------------
echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
