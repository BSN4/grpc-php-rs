<?php
/**
 * Memory leak detection test — with REAL gRPC calls via local test server.
 *
 * Connects to a local insecure gRPC test server on localhost:50051 to exercise
 * the full HTTP/2 + gRPC framing path without external dependencies.
 *
 * Tests exercise every major allocation path:
 *   - Send/receive message buffers at varied sizes
 *   - Metadata header parsing (many headers per call)
 *   - CallCredentials plugin callback invocation
 *   - Error/status response handling
 *   - Channel + Call object churn (create/destroy per call)
 *   - Mixed workload (alternating RPC types)
 *
 * Run:
 *   ./test.sh leak
 *
 * Or manually:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/leak_test.php
 */

echo "=== grpc-php-rs Memory Leak Test (local server) ===\n\n";

$baseline = memory_get_usage();
echo "Baseline memory: " . format_bytes($baseline) . "\n";

// ── Helpers ──

/**
 * Encode a protobuf Payload message: field 1 (bytes) = \x0a + varint len + data
 */
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

/**
 * Make a unary gRPC call using the low-level Grpc\Call + startBatch API.
 * Returns [response_bytes|null, status_code, status_details].
 */
function grpc_call(
    Grpc\Channel $channel,
    string $method,
    string $payload,
    array $metadata = [],
    ?Grpc\CallCredentials $callCreds = null,
): array {
    $call = new Grpc\Call($channel, $method, Grpc\Timeval::infFuture());

    if ($callCreds !== null) {
        $call->setCredentials($callCreds);
    }

    // Send: initial metadata + message + close
    $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => $metadata,
        Grpc\OP_SEND_MESSAGE          => $payload,
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    ]);

    // Receive: metadata + message + status
    $result = $call->startBatch([
        Grpc\OP_RECV_INITIAL_METADATA  => true,
        Grpc\OP_RECV_MESSAGE           => true,
        Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
    ]);

    $status = $result->status ?? null;
    $code = $status->code ?? -1;
    $details = $status->details ?? '';
    $message = $result->message ?? null;

    return [$message, $code, $details];
}

$failed = false;

// ── Connect to local test server (insecure) ──
$target = 'localhost:50051';
echo "Connecting to $target (insecure)...\n";

$channelOpts = [];
$smallPayload = encode_payload('ping');

// ── Test 1: Echo sustained (20K iters, 4KB payload) ──
echo "\n--- Test 1: Echo RPC sustained (20000 iterations, 4KB payload) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$payload4k = encode_payload(str_repeat('A', 4096));
$samples = [];

for ($i = 0; $i < 20000; $i++) {
    [$resp, $code, $details] = grpc_call($channel, '/grpc.testing.TestService/Echo', $payload4k);
    unset($resp);

    if ($i % 4000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel);
$failed = report_samples("Echo RPC sustained (4KB)", $samples, 4000) || $failed;

// ── Test 2: Varied payload sizes (20K iters, rotating 64B / 1KB / 8KB / 32KB) ──
echo "\n--- Test 2: Varied payload sizes (20000 iterations) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$payloads = [
    encode_payload(str_repeat('a', 64)),
    encode_payload(str_repeat('b', 1024)),
    encode_payload(str_repeat('c', 8192)),
    encode_payload(str_repeat('d', 32768)),
];
$payloadCount = count($payloads);
$samples = [];

for ($i = 0; $i < 20000; $i++) {
    $p = $payloads[$i % $payloadCount];
    [$resp, $code, $details] = grpc_call($channel, '/grpc.testing.TestService/Echo', $p);
    unset($resp);

    if ($i % 4000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel, $payloads);
$failed = report_samples("Varied payloads (64B-32KB)", $samples, 4000) || $failed;

// ── Test 3: Large response (5K iters, 64KB each = ~320MB total received) ──
echo "\n--- Test 3: LargeResponse RPC (5000 iterations, 64KB response, ~320MB total) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$samples = [];

for ($i = 0; $i < 5000; $i++) {
    [$resp, $code, $details] = grpc_call($channel, '/grpc.testing.TestService/LargeResponse', $smallPayload);
    unset($resp);

    if ($i % 1000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel);
$failed = report_samples("LargeResponse RPC (64KB x 5000)", $samples, 1000) || $failed;

// ── Test 4: Empty response (20K iters) ──
echo "\n--- Test 4: EmptyResponse RPC (20000 iterations) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$samples = [];

for ($i = 0; $i < 20000; $i++) {
    [$resp, $code, $details] = grpc_call($channel, '/grpc.testing.TestService/EmptyResponse', $smallPayload);
    unset($resp);

    if ($i % 4000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel);
$failed = report_samples("EmptyResponse RPC", $samples, 4000) || $failed;

// ── Test 5: Error response (20K iters) ──
echo "\n--- Test 5: ErrorResponse RPC (20000 iterations) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$samples = [];

for ($i = 0; $i < 20000; $i++) {
    [$resp, $code, $details] = grpc_call($channel, '/grpc.testing.TestService/ErrorResponse', $smallPayload);
    unset($resp);

    if ($i % 4000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel);
$failed = report_samples("ErrorResponse RPC", $samples, 4000) || $failed;

// ── Test 6: Metadata-heavy (10K iters, 20 headers per call) ──
echo "\n--- Test 6: Metadata-heavy RPC (10000 iterations, 20 headers/call) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$heavyMetadata = [];
for ($h = 0; $h < 20; $h++) {
    $heavyMetadata["x-test-header-$h"] = ["value-" . str_repeat("$h", 64)];
}
$samples = [];

for ($i = 0; $i < 10000; $i++) {
    [$resp, $code, $details] = grpc_call(
        $channel, '/grpc.testing.TestService/Echo', $smallPayload, $heavyMetadata
    );
    unset($resp);

    if ($i % 2000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel, $heavyMetadata);
$failed = report_samples("Metadata-heavy (20 headers/call)", $samples, 2000) || $failed;

// ── Test 7: CallCredentials plugin callback (10K iters) ──
echo "\n--- Test 7: CallCredentials plugin (10000 iterations) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$callCreds = Grpc\CallCredentials::createFromPlugin(function (string $serviceUrl) {
    return [
        'authorization' => ['Bearer test-token-' . substr(md5($serviceUrl), 0, 8)],
        'x-request-id'  => [uniqid('req-', true)],
    ];
});
$samples = [];

for ($i = 0; $i < 10000; $i++) {
    [$resp, $code, $details] = grpc_call(
        $channel, '/grpc.testing.TestService/Echo', $smallPayload, [], $callCreds
    );
    unset($resp);

    if ($i % 2000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel, $callCreds);
$failed = report_samples("CallCredentials plugin callback", $samples, 2000) || $failed;

// ── Test 8: Full lifecycle — new channel per call (10K iters) ──
echo "\n--- Test 8: Full lifecycle - channel per call (10000 iterations) ---\n";
$payload1k = encode_payload(str_repeat('X', 1024));
$samples = [];

for ($i = 0; $i < 10000; $i++) {
    $ch = new Grpc\Channel($target, $channelOpts);
    [$resp, $code, $details] = grpc_call($ch, '/grpc.testing.TestService/Echo', $payload1k);
    $ch->close();
    unset($resp, $ch);

    if ($i % 2000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$failed = report_samples("Full lifecycle (channel+call per iter)", $samples, 2000) || $failed;

// ── Test 9: Mixed workload (20K iters, alternating RPC types) ──
echo "\n--- Test 9: Mixed workload (20000 iterations, alternating RPCs) ---\n";
$channel = new Grpc\Channel($target, $channelOpts);
$methods = [
    '/grpc.testing.TestService/Echo',
    '/grpc.testing.TestService/EmptyResponse',
    '/grpc.testing.TestService/LargeResponse',
    '/grpc.testing.TestService/ErrorResponse',
];
$methodCount = count($methods);
$payload2k = encode_payload(str_repeat('M', 2048));
$samples = [];

for ($i = 0; $i < 20000; $i++) {
    $m = $methods[$i % $methodCount];
    [$resp, $code, $details] = grpc_call($channel, $m, $payload2k);
    unset($resp);

    if ($i % 4000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$channel->close();
unset($channel);
$failed = report_samples("Mixed workload (4 RPC types)", $samples, 4000) || $failed;

// ── Test 10: Object lifecycle — no network (20K iters) ──
echo "\n--- Test 10: Object lifecycle - no network (20000 iterations) ---\n";
$samples = [];
for ($i = 0; $i < 20000; $i++) {
    $sslCreds = Grpc\ChannelCredentials::createSsl();
    $callCreds = Grpc\CallCredentials::createFromPlugin(function ($url) {
        return ['authorization' => ['Bearer token']];
    });
    $composite = Grpc\ChannelCredentials::createComposite($sslCreds, $callCreds);
    $channel = new Grpc\Channel('localhost:443', [
        'credentials' => $composite,
        'grpc.ssl_target_name_override' => 'localhost',
    ]);
    $call = new Grpc\Call($channel, '/test.Service/Method', Grpc\Timeval::infFuture());
    $call->cancel();
    unset($call, $channel, $composite, $callCreds, $sslCreds);

    if ($i % 4000 === 0) {
        gc_collect_cycles();
        $samples[] = memory_get_usage();
    }
}
gc_collect_cycles();
$samples[] = memory_get_usage();
$failed = report_samples("Object lifecycle (no network)", $samples, 4000) || $failed;

// ── Summary ──
$final = memory_get_usage();
echo "\n=== Final memory: " . format_bytes($final) . " (delta from baseline: " . format_bytes($final - $baseline, true) . ") ===\n";

if ($failed) {
    echo "\nWARNING: One or more tests showed memory growth > 8KB — investigate.\n";
    exit(1);
}

echo "\nAll tests passed.\n";

// ──────────────────────────────────────────────────────────────
/** Returns true if the test should be flagged for investigation. */
function report_samples(string $name, array $samples, int $interval): bool {
    echo "  Memory samples:\n";
    foreach ($samples as $idx => $mem) {
        $iter = $idx * $interval;
        $delta = $idx > 0 ? ' (delta: ' . format_bytes($mem - $samples[0], true) . ')' : '';
        echo "    iter $iter: " . format_bytes($mem) . "$delta\n";
    }
    $growth = end($samples) - $samples[0];
    $totalIters = (count($samples) - 1) * $interval;
    $perIter = $totalIters > 0 ? $growth / $totalIters : 0;
    $investigate = abs($growth) >= 8192;
    $verdict = !$investigate
        ? "PASS (no significant growth)"
        : "INVESTIGATE (grew " . format_bytes($growth, true) . ", ~" . round($perIter, 2) . " B/iter)";
    echo "  Result: $verdict\n";
    return $investigate;
}

function format_bytes(int $bytes, bool $signed = false): string {
    $prefix = $signed && $bytes >= 0 ? '+' : '';
    if (abs($bytes) < 1024) return $prefix . $bytes . ' B';
    if (abs($bytes) < 1048576) return $prefix . round($bytes / 1024, 2) . ' KB';
    return $prefix . round($bytes / 1048576, 2) . ' MB';
}
