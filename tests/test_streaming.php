<?php
/**
 * Server streaming test — exercises the multi-startBatch pattern
 * that ServerStreamingCall uses in the grpc/grpc PHP library.
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_streaming.php
 */

echo "=== grpc-php-rs Streaming Tests ===\n\n";

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

// --- Test 2: Client streaming (mimics ClientStreamingCall pattern) ---
echo "\n--- Test 2: Client streaming via split startBatch ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/CollectPayloads', Grpc\Timeval::infFuture());

// Step 1: Send initial metadata only (like ClientStreamingCall::start())
echo "  Sending initial metadata...\n";
$call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
]);

// Step 2: Send multiple messages (like ClientStreamingCall::write())
$parts = ['aaa', 'bbb', 'ccc'];
foreach ($parts as $part) {
    echo "  Writing '$part'...\n";
    $call->startBatch([
        Grpc\OP_SEND_MESSAGE => encode_payload($part),
    ]);
}

// Step 3: Close send + receive response (like ClientStreamingCall::wait())
echo "  Closing send + receiving response...\n";
$result = $call->startBatch([
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    Grpc\OP_RECV_INITIAL_METADATA  => true,
    Grpc\OP_RECV_MESSAGE           => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
]);

$statusCode = $result->status->code ?? -1;
$statusDetails = $result->status->details ?? '';
echo "  Status: code=$statusCode details='$statusDetails'\n";

// The server concatenates all received bodies — should be "aaabbbccc"
$body = $result->message;
if ($body === null) {
    echo "  FAIL: got null response\n";
    $failed = true;
} else {
    // Decode the Payload protobuf: field 1 (bytes) = 0x0a + varint len + data
    // Skip the protobuf framing to get the raw concatenated bytes
    $decoded = substr($body, 2); // skip 0x0a + 1-byte varint length
    echo "  Response body: '$decoded' (" . strlen($decoded) . " bytes)\n";
    if ($decoded === 'aaabbbccc') {
        echo "  PASS: server received all 3 messages\n";
    } else {
        echo "  FAIL: expected 'aaabbbccc', got '$decoded'\n";
        $failed = true;
    }
}

if ($statusCode !== 0) {
    echo "  FAIL: expected status 0 (OK), got $statusCode\n";
    $failed = true;
} else {
    echo "  PASS: status OK\n";
}

// --- Test 3: Bidi streaming (mimics BidiStreamingCall pattern) ---
echo "\n--- Test 3: Bidi streaming via split startBatch ---\n";

$call = new Grpc\Call($channel, '/grpc.testing.TestService/BidiEcho', Grpc\Timeval::infFuture());

// Step 1: Send initial metadata (like BidiStreamingCall::start())
echo "  Sending initial metadata...\n";
$call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA => [],
]);

// Step 2: Interleaved send/recv
$bidiMessages = [];
$bidiParts = ['hello', 'world', 'bidi'];
foreach ($bidiParts as $idx => $part) {
    echo "  Writing '$part'...\n";
    $call->startBatch([
        Grpc\OP_SEND_MESSAGE => encode_payload($part),
    ]);

    $recvOps = [Grpc\OP_RECV_MESSAGE => true];
    if ($idx === 0) {
        $recvOps[Grpc\OP_RECV_INITIAL_METADATA] = true;
    }
    echo "  Reading response $idx...\n";
    $result = $call->startBatch($recvOps);

    if ($result->message !== null) {
        $bidiMessages[] = $result->message;
        echo "  Got message: " . strlen($result->message) . " bytes\n";
    } else {
        echo "  Got null message (unexpected)\n";
    }
}

// Step 3: Close send (like BidiStreamingCall::writesDone())
echo "  Closing send...\n";
$call->startBatch([
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
]);

// Step 4: Read remaining messages until end of stream
for ($i = 0; $i < 10; $i++) {
    $result = $call->startBatch([
        Grpc\OP_RECV_MESSAGE => true,
    ]);
    if ($result->message === null) {
        echo "  End of stream\n";
        break;
    }
    $bidiMessages[] = $result->message;
    echo "  Got extra message: " . strlen($result->message) . " bytes\n";
}

// Step 5: Receive status
echo "  Receiving status...\n";
$result = $call->startBatch([
    Grpc\OP_RECV_STATUS_ON_CLIENT => true,
]);
$statusCode = $result->status->code ?? -1;
$statusDetails = $result->status->details ?? '';
echo "  Status: code=$statusCode details='$statusDetails'\n";

if (count($bidiMessages) !== 3) {
    echo "  FAIL: expected 3 echoed messages, got " . count($bidiMessages) . "\n";
    $failed = true;
} else {
    echo "  PASS: received 3 echoed messages\n";
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
    echo "FAILED: Streaming tests did not pass.\n";
    exit(1);
}

echo "All streaming tests passed.\n";
