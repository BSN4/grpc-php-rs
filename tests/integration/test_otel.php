<?php
declare(strict_types=1);

// Tests the ext-grpc API surface against a real OpenTelemetry Collector.
// Exercises the same extension calls made by open-telemetry/transport-grpc:
//   Grpc\Channel, Grpc\ChannelCredentials::createInsecure(), Grpc\Timeval,
//   Grpc\Call, startBatch(), OP_* constants.

$tests  = 0;
$passed = 0;

function check(string $name, bool $result, string $extra = ''): void {
    global $tests, $passed;
    $tests++;
    if ($result) {
        $passed++;
        echo "  \u{2713} {$name}\n";
    } else {
        $suffix = $extra !== '' ? " ({$extra})" : '';
        echo "  \u{2717} {$name}{$suffix}\n";
    }
}

$address = getenv('OTEL_ADDRESS') ?: 'otel-collector:4317';

echo "=== OpenTelemetry gRPC Integration Test ===\n";
echo "    Collector: {$address}\n\n";

// Retry loop: collector may still be initialising when the container starts.
$max_attempts = 10;
$attempt      = 0;
$statusCode   = -1;
$statusDetails = '';
$channel = null;
$event   = null;

while ($attempt < $max_attempts) {
    $attempt++;
    $channel = new Grpc\Channel($address, [
        'credentials' => Grpc\ChannelCredentials::createInsecure(),
    ]);
    $deadline = new Grpc\Timeval(3_000_000); // 3-second per-attempt deadline
    $call = new Grpc\Call(
        $channel,
        '/opentelemetry.proto.collector.trace.v1.TraceService/Export',
        $deadline,
    );
    $event = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA  => [],
        Grpc\OP_SEND_MESSAGE           => ['message' => ''],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
        Grpc\OP_RECV_INITIAL_METADATA  => true,
        Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
        Grpc\OP_RECV_MESSAGE           => true,
    ]);
    $channel->close();

    $statusCode    = $event->status->code ?? -1;
    $statusDetails = $event->status->details ?? '';

    // UNAVAILABLE (14) usually means collector not ready yet; retry.
    if ($statusCode !== Grpc\STATUS_UNAVAILABLE) {
        break;
    }
    echo "    attempt {$attempt}/{$max_attempts}: collector not ready, retrying...\n";
    sleep(1);
}

check('Channel created with createInsecure()', $channel !== null);
check(
    "TraceService/Export status OK",
    $statusCode === Grpc\STATUS_OK,
    "code={$statusCode}" . ($statusDetails !== '' ? " details={$statusDetails}" : ''),
);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
