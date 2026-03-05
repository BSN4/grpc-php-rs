<?php
declare(strict_types=1);

// Tests the ext-grpc API surface against a real Temporal server.
// Exercises the same extension calls made by temporal/sdk ServiceClient.

require_once '/integration/vendor/autoload.php';

use Temporal\Client\GRPC\ServiceClient;

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

$address = getenv('TEMPORAL_ADDRESS') ?: 'temporal:7233';

echo "=== Temporal gRPC Integration Test ===\n";
echo "    Server: {$address}\n\n";

// ServiceClient::create() calls Grpc\ChannelCredentials::createInsecure()
// and instantiates Grpc\BaseStub (WorkflowServiceClient).
$client = ServiceClient::create($address);
check('ServiceClient created', $client instanceof ServiceClient);

// GetSystemInfo exercises the same ext-grpc primitives as temporal/sdk:
//   Grpc\Channel, Grpc\ChannelCredentials::createInsecure(), Grpc\Call,
//   Grpc\Timeval, Grpc\BaseStub::_simpleRequest(), Grpc\UnaryCall::wait()
//
// We use the raw ext-grpc API directly with an explicit deadline so the test
// always terminates, regardless of extension behaviour.
$call_deadline_us = 5_000_000; // 5 seconds in microseconds
$channel = new Grpc\Channel($address, [
    'credentials' => Grpc\ChannelCredentials::createInsecure(),
]);
$deadline = new Grpc\Timeval($call_deadline_us);
$call = new Grpc\Call(
    $channel,
    '/temporal.api.workflowservice.v1.WorkflowService/GetSystemInfo',
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

check(
    "GetSystemInfo() status OK",
    $statusCode === Grpc\STATUS_OK,
    "code={$statusCode}" . ($statusDetails !== '' ? " details={$statusDetails}" : ''),
);
check(
    "GetSystemInfo() returned message bytes",
    strlen($event->message ?? '') > 0,
);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
