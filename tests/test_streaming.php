<?php
/**
 * Server streaming test — exercises the multi-startBatch pattern
 * that ServerStreamingCall uses in the grpc/grpc PHP library.
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_streaming.php
 */

echo "=== grpc-php-rs Server Streaming Test ===\n\n";

// Protobuf helpers (same as leak_test.php)
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

// --- Test 1: Server streaming (mimics ServerStreamingCall pattern) ---
echo "--- Test 1: Server streaming via split startBatch ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/StreamEcho', Grpc\Timeval::infFuture());

// Step 1: Send request (send-only batch, like ServerStreamingCall::start())
echo "  Sending request...\n";
$call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
    Grpc\OP_SEND_MESSAGE          => encode_payload('hello-stream'),
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
]);

// Step 2: Receive initial metadata (like ServerStreamingCall::responses() first call)
echo "  Receiving initial metadata...\n";
$result = $call->startBatch([
    Grpc\OP_RECV_INITIAL_METADATA => true,
]);
echo "  Got metadata: " . (isset($result->metadata) ? "yes" : "no") . "\n";

// Step 3: Read streamed messages in a loop (like ServerStreamingCall::responses() iteration)
$messages = [];
$maxReads = 10; // safety limit
for ($i = 0; $i < $maxReads; $i++) {
    echo "  Reading message $i...\n";
    $result = $call->startBatch([
        Grpc\OP_RECV_MESSAGE => true,
    ]);

    if ($result->message === null) {
        echo "  Got null message (end of stream)\n";
        break;
    }

    $messages[] = $result->message;
    echo "  Got message: " . strlen($result->message) . " bytes\n";
}

// Step 4: Receive status (like ServerStreamingCall::getStatus())
echo "  Receiving status...\n";
$result = $call->startBatch([
    Grpc\OP_RECV_STATUS_ON_CLIENT => true,
]);
$statusCode = $result->status->code ?? -1;
$statusDetails = $result->status->details ?? '';
echo "  Status: code=$statusCode details='$statusDetails'\n";

// Verify results
$expectedCount = 3; // server sends 3 copies
if (count($messages) !== $expectedCount) {
    echo "  FAIL: expected $expectedCount messages, got " . count($messages) . "\n";
    $failed = true;
} else {
    echo "  PASS: received $expectedCount streamed messages\n";
}

if ($statusCode !== 0) {
    echo "  FAIL: expected status 0 (OK), got $statusCode\n";
    $failed = true;
} else {
    echo "  PASS: status OK\n";
}

$channel->close();

// --- Summary ---
echo "\n";
if ($failed) {
    echo "FAILED: Server streaming does not work correctly.\n";
    exit(1);
}

echo "All streaming tests passed.\n";
