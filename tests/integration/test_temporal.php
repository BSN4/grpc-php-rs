<?php
declare(strict_types=1);

// Tests the ext-grpc API surface against a real Temporal server.
// Exercises the same extension calls made by temporal/sdk ServiceClient.

require_once '/integration/vendor/autoload.php';

use Temporal\Client\GRPC\ServiceClient;
use Temporal\Api\WorkflowService\V1\GetSystemInfoRequest;

$tests  = 0;
$passed = 0;

function check(string $name, bool $result): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}\n"; }
}

$address = getenv('TEMPORAL_ADDRESS') ?: 'temporal:7233';

echo "=== Temporal gRPC Integration Test ===\n";
echo "    Server: {$address}\n\n";

// ServiceClient::create() calls Grpc\ChannelCredentials::createInsecure()
// and instantiates Grpc\BaseStub (WorkflowServiceClient)
$client = ServiceClient::create($address);
check('ServiceClient created', $client instanceof ServiceClient);

// GetSystemInfo() exercises Grpc\UnaryCall via BaseStub::_simpleRequest()
try {
    $response = $client->GetSystemInfo(new GetSystemInfoRequest());
    check('GetSystemInfo() returned response', $response !== null);
    check('Response is GetSystemInfoResponse', $response instanceof \Temporal\Api\WorkflowService\V1\GetSystemInfoResponse);
} catch (Throwable $e) {
    check('GetSystemInfo() no exception: ' . $e->getMessage(), false);
}

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
