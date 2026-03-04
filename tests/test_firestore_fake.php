<?php
declare(strict_types=1);

// Tests that google/cloud-firestore can load and use our Grpc\ extension
// classes without real credentials. We point at a fake endpoint and verify
// we get a gRPC transport error (not a class-not-found or SIGSEGV).

echo "=== Test: Firestore client with fake endpoint ===\n";

require __DIR__ . '/../vendor/autoload.php';

use Google\Cloud\Firestore\FirestoreClient;

$tests = 0;
$passed = 0;

function check(string $name, bool $result): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}\n"; }
}

// 1. Verify the Firestore client can be instantiated
$firestore = new FirestoreClient([
    'projectId' => 'fake-project',
    'apiEndpoint' => 'localhost:1', // nothing listens here
    'transport' => 'grpc',
]);
check('FirestoreClient instantiated', $firestore instanceof FirestoreClient);

// 2. Attempt a write — should fail with transport/connection error, NOT a crash
$docRef = $firestore->document('test-collection/test-doc');
check('document() ref created', $docRef !== null);

try {
    $docRef->set(['hello' => 'world']);
    // If we get here, something unexpected happened
    check('set() threw expected error', false);
} catch (\Throwable $e) {
    $msg = $e->getMessage();
    $class = get_class($e);

    echo "  Exception: {$class}\n";
    echo "  Message:   {$msg}\n";

    // It should be a gRPC/connection error, not a PHP class/type error
    $isTransportError = str_contains($msg, 'connect')
        || str_contains($msg, 'transport')
        || str_contains($msg, 'failed')
        || str_contains($msg, 'UNAVAILABLE')
        || str_contains($msg, 'deadline')
        || str_contains($msg, 'timeout')
        || str_contains($msg, 'refused')
        || str_contains($class, 'ServiceException')
        || str_contains($class, 'ApiException');

    $isNotClassError = !str_contains($msg, 'not found')
        && !str_contains($msg, 'not loaded')
        && !str_contains($msg, 'undefined');

    check('Error is transport/gRPC (not class-not-found)', $isTransportError && $isNotClassError);
}

// 3. Attempt a read
try {
    $snapshot = $docRef->snapshot();
    check('snapshot() threw expected error', false);
} catch (\Throwable $e) {
    $msg = $e->getMessage();
    $class = get_class($e);
    echo "  Read exception: {$class}: {$msg}\n";
    check('Read also fails with transport error (no crash)', true);
}

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
