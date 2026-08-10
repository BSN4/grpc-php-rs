<?php
/**
 * Compatibility test with grpc/grpc PHP library.
 *
 * Verifies that grpc/grpc classes (InterceptorChannel, BaseStub, etc.)
 * can extend and override Grpc\Channel methods without fatal errors.
 * This catches return type covariance issues (PHP 8.5+).
 */

// grpc/grpc must be installed via Composer
require_once __DIR__ . '/../vendor/autoload.php';

$pass = 0;
$fail = 0;
$errors = [];

function test(string $name, callable $fn): void {
    global $pass, $fail, $errors;
    try {
        $fn();
        echo "  PASS: {$name}\n";
        $pass++;
    } catch (\Throwable $e) {
        echo "  FAIL: {$name}\n";
        echo "        " . get_class($e) . ": " . $e->getMessage() . "\n";
        $fail++;
        $errors[] = $name;
    }
}

echo "=== grpc/grpc Compatibility Tests ===\n\n";

// Test 1: InterceptorChannel can be instantiated
// This tests that the class can extend Grpc\Channel without fatal errors
test('InterceptorChannel class loads', function () {
    $rc = new ReflectionClass('Grpc\Internal\InterceptorChannel');
    if (!$rc->isSubclassOf('Grpc\Channel')) throw new RuntimeException('InterceptorChannel must extend Grpc\Channel');
});

// Test 2: InterceptorChannel::getTarget() override is compatible
test('InterceptorChannel::getTarget() override', function () {
    $channel = new \Grpc\Channel('localhost:50051', [
        'credentials' => \Grpc\ChannelCredentials::createInsecure(),
    ]);
    // Create a no-op interceptor
    $interceptor = new class extends \Grpc\Interceptor {};
    $ic = new \Grpc\Internal\InterceptorChannel($channel, $interceptor);
    $target = $ic->getTarget();
    if (!(is_string($target))) throw new RuntimeException('getTarget() should return a string');
    $channel->close();
});

// Test 3: InterceptorChannel::getConnectivityState() override is compatible
test('InterceptorChannel::getConnectivityState() override', function () {
    $channel = new \Grpc\Channel('localhost:50051', [
        'credentials' => \Grpc\ChannelCredentials::createInsecure(),
    ]);
    $interceptor = new class extends \Grpc\Interceptor {};
    $ic = new \Grpc\Internal\InterceptorChannel($channel, $interceptor);
    $state = $ic->getConnectivityState();
    if (!(is_int($state))) throw new RuntimeException('getConnectivityState() should return an int');
    $channel->close();
});

// Test 4: InterceptorChannel::close() override is compatible
test('InterceptorChannel::close() override', function () {
    $channel = new \Grpc\Channel('localhost:50051', [
        'credentials' => \Grpc\ChannelCredentials::createInsecure(),
    ]);
    $interceptor = new class extends \Grpc\Interceptor {};
    $ic = new \Grpc\Internal\InterceptorChannel($channel, $interceptor);
    $ic->close();
});

// Test 5: BaseStub can be instantiated with InterceptorChannel
test('BaseStub with InterceptorChannel', function () {
    $channel = new \Grpc\Channel('localhost:50051', [
        'credentials' => \Grpc\ChannelCredentials::createInsecure(),
    ]);
    $interceptor = new class extends \Grpc\Interceptor {};
    $ic = new \Grpc\Internal\InterceptorChannel($channel, $interceptor);

    // The stub must be constructed OVER the InterceptorChannel — that's the
    // interaction issue #4 broke; constructing it standalone tests nothing.
    $stub = new class('localhost:50051', [
        'credentials' => \Grpc\ChannelCredentials::createInsecure(),
    ], $ic) extends \Grpc\BaseStub {
    };
    if (!is_string($stub->getTarget()) || $stub->getTarget() === '') {
        throw new RuntimeException('BaseStub-over-InterceptorChannel getTarget() should work');
    }
    $channel->close();
});

// Test 6: Reflection check — Grpc\Channel methods should have no declared return types
// (matching the C extension behavior)
test('Grpc\Channel methods have no return types', function () {
    $rc = new ReflectionClass('Grpc\Channel');
    $methods = ['getTarget', 'getConnectivityState', 'watchConnectivityState', 'close'];
    foreach ($methods as $name) {
        if ($rc->hasMethod($name)) {
            $rm = $rc->getMethod($name);
            if ($rm->hasReturnType()) {
                throw new \RuntimeException(
                    "{$name}() has return type :{$rm->getReturnType()}, should be untyped"
                );
            }
        }
    }
});

echo "\n=== Results: {$pass} passed, {$fail} failed ===\n";
if ($fail > 0) {
    echo "Failed tests: " . implode(', ', $errors) . "\n";
    exit(1);
}
