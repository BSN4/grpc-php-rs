<?php
declare(strict_types=1);

// Requires: composer require google/cloud-firestore
// Requires: GOOGLE_APPLICATION_CREDENTIALS env var pointing to service account JSON
// OR: GOOGLE_CLOUD_PROJECT + running on GCE/Cloud Run with default credentials

echo "=== Test: Firestore set() via gRPC ===\n";

require __DIR__ . '/../vendor/autoload.php';

use Google\Cloud\Firestore\FirestoreClient;

$projectId = getenv('GOOGLE_CLOUD_PROJECT') ?: getenv('GCLOUD_PROJECT');
if (!$projectId) {
    echo "Set GOOGLE_CLOUD_PROJECT env var\n";
    exit(1);
}

$firestore = new FirestoreClient([
    'projectId' => $projectId,
]);

$testDoc = 'grpc-php-rs-test/smoke-' . bin2hex(random_bytes(4));
echo "  Writing to: {$testDoc}\n";

// This is THE critical call — it triggers:
// 1. CallCredentials plugin callback (OAuth2 token fetch)
// 2. TLS handshake via rustls (not OpenSSL)
// 3. HTTP/2 framing via hyper
// 4. Protobuf serialization over gRPC UnaryCall
$docRef = $firestore->document($testDoc);
$docRef->set([
    'test'      => true,
    'timestamp' => time(),
    'source'    => 'grpc-php-rs',
]);
echo "  ✓ set() succeeded — no SIGSEGV!\n";

// Read it back to verify
$snapshot = $docRef->snapshot();
assert($snapshot->exists(), 'Document should exist');
assert($snapshot['source'] === 'grpc-php-rs', 'Data mismatch');
echo "  ✓ get() verified — data matches\n";

// Clean up
$docRef->delete();
echo "  ✓ delete() succeeded\n";

echo "\n  ✓ All Firestore gRPC tests passed\n";
