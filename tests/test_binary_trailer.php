<?php
/**
 * Binary trailing metadata test. Exercises the `-bin` trailer code path
 * that this PR (binary-metadata surfacing) restores.
 *
 * The Rust test server attaches a binary trailer `x-test-binary-bin` to the
 * Echo response. This test asserts the bytes are surfaced to PHP intact
 * (no UTF-8 conversion, NUL and non-UTF-8 bytes preserved).
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_binary_trailer.php
 */
declare(strict_types=1);

echo "=== Binary Trailing Metadata Test ===\n\n";

function encode_payload(string $data): string {
    if ($data === '') return '';
    return "\x0a" . encode_varint(strlen($data)) . $data;
}

function encode_varint(int $value): string {
    $buf = '';
    while ($value > 0x7f) {
        $buf .= chr(($value & 0x7f) | 0x80);
        $value >>= 7;
    }
    $buf .= chr($value & 0x7f);
    return $buf;
}

$tests = 0;
$passed = 0;
function check(string $name, bool $result, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

$expected = "hello\x00\xfftrailer";  // matches tests/server/src/main.rs
$target = 'localhost:50051';
$channel = new Grpc\Channel($target, []);

// -----------------------------------------------------------------------------
// Case 1: Echo response, binary metadata via Response::metadata_mut().
// In tonic 0.12, metadata_mut() on a successful unary response populates the
// initial-metadata HEADERS frame. Verifies our metadata_to_php fix on the
// initial-metadata receive path.
// -----------------------------------------------------------------------------
echo "--- Case 1: success response, binary header ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/Echo', Grpc\Timeval::infFuture());
$r1 = $call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
    Grpc\OP_SEND_MESSAGE => encode_payload('hi'),
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    Grpc\OP_RECV_INITIAL_METADATA => true,
    Grpc\OP_RECV_MESSAGE => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT => true,
]);

check('echo: status OK', ($r1->status->code ?? -1) === Grpc\STATUS_OK);
$initial = $r1->metadata ?? [];
check('echo: x-test-binary-bin in initial metadata',
    is_array($initial) && array_key_exists('x-test-binary-bin', $initial),
    'keys=' . (is_array($initial) ? implode(',', array_keys($initial)) : 'null'));
$got = $initial['x-test-binary-bin'][0] ?? '';
check('echo: byte-exact match (NUL + non-UTF-8 preserved)', $got === $expected,
    'expected=' . bin2hex($expected) . ' got=' . bin2hex($got));

// -----------------------------------------------------------------------------
// Case 2: Error response, rich-status bytes attached via Status::with_details.
// tonic parses the literal reserved `grpc-status-details-bin` header into
// Status::details(), so this verifies grpc-php-rs restores it to trailing
// metadata on the PHP receive path.
// -----------------------------------------------------------------------------
echo "\n--- Case 2: error response, binary trailer ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/ErrorResponse', Grpc\Timeval::infFuture());
$r2 = $call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
    Grpc\OP_SEND_MESSAGE => encode_payload('hi'),
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    Grpc\OP_RECV_INITIAL_METADATA => true,
    Grpc\OP_RECV_MESSAGE => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT => true,
]);

check('error: status INTERNAL', ($r2->status->code ?? -1) === Grpc\STATUS_INTERNAL,
    'code=' . var_export($r2->status->code ?? null, true));
$trailers = $r2->status->metadata ?? [];
check('error: grpc-status-details-bin in trailing metadata',
    is_array($trailers) && array_key_exists('grpc-status-details-bin', $trailers),
    'keys=' . (is_array($trailers) ? implode(',', array_keys($trailers)) : 'null'));
$got = $trailers['grpc-status-details-bin'][0] ?? '';
check('error: byte-exact match (NUL + non-UTF-8 preserved)', $got === $expected,
    'expected=' . bin2hex($expected) . ' got=' . bin2hex($got));

// -----------------------------------------------------------------------------
// Case 3: Server-streaming error after response headers. This takes tonic's
// `Streaming::message()` error path and ensures rich status is restored to the
// metadata that RECV_STATUS_ON_CLIENT receives.
// -----------------------------------------------------------------------------
echo "\n--- Case 3: streaming error, rich-status trailer ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/StreamEcho', Grpc\Timeval::infFuture());
$call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
    Grpc\OP_SEND_MESSAGE => encode_payload('stream-status-error'),
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
]);
$call->startBatch([
    Grpc\OP_RECV_INITIAL_METADATA => true,
]);
$streamMessage = $call->startBatch([
    Grpc\OP_RECV_MESSAGE => true,
]);
check('streaming error: message is null', $streamMessage->message === null);
$streamStatus = $call->startBatch([
    Grpc\OP_RECV_STATUS_ON_CLIENT => true,
]);
check('streaming error: status INTERNAL', ($streamStatus->status->code ?? -1) === Grpc\STATUS_INTERNAL,
    'code=' . var_export($streamStatus->status->code ?? null, true));
$streamTrailers = $streamStatus->status->metadata ?? [];
check('streaming error: grpc-status-details-bin in trailing metadata',
    is_array($streamTrailers) && array_key_exists('grpc-status-details-bin', $streamTrailers),
    'keys=' . (is_array($streamTrailers) ? implode(',', array_keys($streamTrailers)) : 'null'));
$got = $streamTrailers['grpc-status-details-bin'][0] ?? '';
check('streaming error: byte-exact match (NUL + non-UTF-8 preserved)', $got === $expected,
    'expected=' . bin2hex($expected) . ' got=' . bin2hex($got));

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
