<?php
declare(strict_types=1);

// Tests that google/grpc-gcp can load and use our Grpc\ extension
// classes without errors. Verifies channel pool management, call invoker,
// and GcpExtensionChannel work with our Channel/Call implementations.
//
// Key failure mode tested:
//   Fatal error: Class "Grpc\BaseStub" not found
//   in google/gax GrpcTransport.php (extends Grpc\BaseStub)

echo "=== Test: google/grpc-gcp compatibility ===\n";

require __DIR__ . '/../vendor/autoload.php';

$tests = 0;
$passed = 0;

function check(string $name, bool $result): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}\n"; }
}

// --- Critical: BaseStub must exist (provided by grpc/grpc Composer package) ---
// This is the class that google/gax's GrpcTransport extends.
// If missing: Fatal error: Class "Grpc\BaseStub" not found
check('Grpc\BaseStub class exists', class_exists('Grpc\BaseStub'));

// BaseStub instantiation (the actual failure point from the error)
$insecureForStub = Grpc\ChannelCredentials::createInsecure();
$stub = new Grpc\BaseStub('localhost:50051', [
    'credentials' => $insecureForStub,
]);
check('BaseStub instantiated with insecure channel', $stub instanceof Grpc\BaseStub);
check('BaseStub::getTarget() works', str_contains($stub->getTarget(), 'localhost'));
check('BaseStub::getConnectivityState() works', is_int($stub->getConnectivityState()));
$stub->close();
check('BaseStub::close() no crash', true);

// grpc/grpc PHP classes that Google Cloud SDKs depend on
check('Grpc\UnaryCall exists', class_exists('Grpc\UnaryCall'));
check('Grpc\ServerStreamingCall exists', class_exists('Grpc\ServerStreamingCall'));
check('Grpc\ClientStreamingCall exists', class_exists('Grpc\ClientStreamingCall'));
check('Grpc\BidiStreamingCall exists', class_exists('Grpc\BidiStreamingCall'));
check('Grpc\CallInvoker interface exists', interface_exists('Grpc\CallInvoker'));
check('Grpc\DefaultCallInvoker exists', class_exists('Grpc\DefaultCallInvoker'));
check('Grpc\Interceptor exists', class_exists('Grpc\Interceptor'));
check('Grpc\Internal\InterceptorChannel exists', class_exists('Grpc\Internal\InterceptorChannel'));

// google/gax GrpcTransport (extends BaseStub — the exact class from the error)
if (class_exists('Google\ApiCore\Transport\GrpcTransport')) {
    check('GrpcTransport class loaded (extends BaseStub)', true);
    $rc = new ReflectionClass('Google\ApiCore\Transport\GrpcTransport');
    check('GrpcTransport parent is BaseStub', $rc->getParentClass()->getName() === 'Grpc\BaseStub');
} else {
    echo "  ! GrpcTransport not available (google/gax not installed)\n";
}

// 1. Verify core classes exist
check('GcpExtensionChannel class exists', class_exists('Grpc\Gcp\GcpExtensionChannel'));
check('GCPCallInvoker class exists', class_exists('Grpc\Gcp\GCPCallInvoker'));
check('ChannelRef class exists', class_exists('Grpc\Gcp\ChannelRef'));
check('GCPUnaryCall class exists', class_exists('Grpc\Gcp\GCPUnaryCall'));
check('GCPServerStreamCall class exists', class_exists('Grpc\Gcp\GCPServerStreamCall'));
check('GCPClientStreamCall class exists', class_exists('Grpc\Gcp\GCPClientStreamCall'));
check('GCPBidiStreamingCall class exists', class_exists('Grpc\Gcp\GCPBidiStreamingCall'));

// 2. GCPCallInvoker implements \Grpc\CallInvoker
check('GCPCallInvoker implements Grpc\CallInvoker', is_a('Grpc\Gcp\GCPCallInvoker', 'Grpc\CallInvoker', true));

// 3. Create a GcpExtensionChannel with insecure credentials
$insecure = Grpc\ChannelCredentials::createInsecure();
$gcpChannel = new Grpc\Gcp\GcpExtensionChannel('localhost:50051', [
    'credentials' => $insecure,
]);
check('GcpExtensionChannel instantiated', $gcpChannel instanceof Grpc\Gcp\GcpExtensionChannel);

// 4. Verify it created internal channel refs
check('channel_refs populated', count($gcpChannel->channel_refs) > 0);

// 5. getTarget() works
check('getTarget() returns target', $gcpChannel->getTarget() === 'localhost:50051');

// 6. getConnectivityState() works (uses our Channel internally)
$state = $gcpChannel->getConnectivityState();
check('getConnectivityState() returns int', is_int($state));

// 7. close() works without crash
$gcpChannel->close();
check('close() no crash', true);

// 8. After close, getConnectivityState should throw
try {
    $gcpChannel->getConnectivityState();
    check('getConnectivityState() after close throws', false);
} catch (\RuntimeException $e) {
    check('getConnectivityState() after close throws', str_contains($e->getMessage(), 'closed'));
}

// 9. GCPCallInvoker creates channel factory
$callInvoker = new Grpc\Gcp\GCPCallInvoker([]);
$factory = $callInvoker->createChannelFactory('localhost:50051', [
    'credentials' => $insecure,
]);
check('createChannelFactory() returns GcpExtensionChannel', $factory instanceof Grpc\Gcp\GcpExtensionChannel);

// 10. Call invoker creates call objects
$unary = $callInvoker->UnaryCall($factory, '/test/Method', 'Grpc\Gcp\GCPUnaryCall', []);
check('UnaryCall() returns GCPUnaryCall', $unary instanceof Grpc\Gcp\GCPUnaryCall);

$serverStream = $callInvoker->ServerStreamingCall($factory, '/test/Method', 'Grpc\Gcp\GCPServerStreamCall', []);
check('ServerStreamingCall() returns GCPServerStreamCall', $serverStream instanceof Grpc\Gcp\GCPServerStreamCall);

$clientStream = $callInvoker->ClientStreamingCall($factory, '/test/Method', 'Grpc\Gcp\GCPClientStreamCall', []);
check('ClientStreamingCall() returns GCPClientStreamCall', $clientStream instanceof Grpc\Gcp\GCPClientStreamCall);

$bidi = $callInvoker->BidiStreamingCall($factory, '/test/Method', 'Grpc\Gcp\GCPBidiStreamingCall', []);
check('BidiStreamingCall() returns GCPBidiStreamingCall', $bidi instanceof Grpc\Gcp\GCPBidiStreamingCall);

// 11. Multiple channels in pool
$gcpChannel2 = new Grpc\Gcp\GcpExtensionChannel('localhost:50052', [
    'credentials' => $insecure,
    'affinity_conf' => [
        'channelPool' => ['maxSize' => 3],
        'affinity_by_method' => [],
    ],
]);
check('GcpExtensionChannel with pool config', $gcpChannel2->max_size === 3);
$gcpChannel2->close();
check('Pool channel close() no crash', true);

// Clean up call invoker channel
$factory->close();

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
