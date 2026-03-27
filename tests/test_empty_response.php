<?php
/**
 * Empty response test — verifies that RPCs returning 0-byte protobuf
 * messages (e.g. google.protobuf.Empty) return an empty string, not null.
 *
 * The C-based grpc extension returns "" for empty bodies, and Google Cloud
 * PHP libraries depend on that behavior.
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_empty_response.php
 */

echo "=== grpc-php-rs Empty Response Test ===\n\n";

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

$target = 'localhost:50051';
$channel = new Grpc\Channel($target, []);
$failed = false;

// --- Test 1: EmptyResponse RPC should return empty string, not null ---
echo "--- Test 1: EmptyResponse RPC returns empty string ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/EmptyResponse', Grpc\Timeval::infFuture());

$result = $call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA  => [],
    Grpc\OP_SEND_MESSAGE           => encode_payload('test'),
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    Grpc\OP_RECV_INITIAL_METADATA  => true,
    Grpc\OP_RECV_MESSAGE           => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
]);

$statusCode = $result->status->code ?? -1;
$message = $result->message;

if ($statusCode !== 0) {
    echo "  FAIL: expected status 0 (OK), got $statusCode\n";
    $failed = true;
} else {
    echo "  PASS: status OK\n";
}

if ($message === "") {
    echo "  PASS: message is empty string\n";
} else {
    echo "  FAIL: expected empty string, got " . ($message === null ? "null" : bin2hex($message)) . "\n";
    $failed = true;
}

$channel->close();

// --- Summary ---
echo "\n";
if ($failed) {
    echo "FAILED: Empty response handling is broken.\n";
    exit(1);
}

echo "All empty response tests passed.\n";
