<?php
declare(strict_types=1);

// Tests the ext-grpc API surface against a real OTLP collector.
// Exercises the same extension calls made by open-telemetry/transport-grpc GrpcTransport.

$tests  = 0;
$passed = 0;

function check(string $name, bool $result): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}\n"; }
}

$address = getenv('OTEL_ADDRESS') ?: 'otel-collector:4317';

echo "=== OpenTelemetry OTLP Integration Test ===\n";
echo "    Endpoint: {$address}\n\n";

// Create insecure channel — mirrors GrpcTransportFactory::create() for http:// endpoints
$channel = new Grpc\Channel($address, ['credentials' => Grpc\ChannelCredentials::createInsecure()]);
check('Channel created', $channel instanceof Grpc\Channel);

// 5-second deadline — mirrors GrpcTransportFactory default timeout
$deadline = new Grpc\Timeval(5_000_000);
check('Timeval created', $deadline instanceof Grpc\Timeval);

// Build the call — same method path used by open-telemetry/transport-grpc
$call = new Grpc\Call(
    $channel,
    '/opentelemetry.proto.collector.trace.v1.TraceService/Export',
    $deadline,
);
check('Call created', $call instanceof Grpc\Call);

// startBatch — this is the exact call sequence in GrpcTransport::send()
// An empty ExportTraceServiceRequest is valid proto3 (all fields are optional)
$event = $call->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA  => [],
    Grpc\OP_SEND_MESSAGE           => ['message' => ''],
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    Grpc\OP_RECV_INITIAL_METADATA  => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
    Grpc\OP_RECV_MESSAGE           => true,
]);
check('startBatch() returned result', $event !== false);
check('STATUS_OK from collector', isset($event->status->code) && $event->status->code === Grpc\STATUS_OK);

$channel->close();
check('Channel::close() no error', true);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
