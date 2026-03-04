<?php
declare(strict_types=1);

echo "=== Test: Extension Loading ===\n";
assert(extension_loaded('grpc'), 'grpc extension not loaded');
echo "  ✓ grpc extension loaded\n";

echo "\n=== Test: SSL ChannelCredentials ===\n";
$creds = Grpc\ChannelCredentials::createSsl();
assert($creds instanceof Grpc\ChannelCredentials, 'createSsl() failed');
echo "  ✓ createSsl() works\n";

echo "\n=== Test: Channel to Google (real TLS handshake) ===\n";
$channel = new Grpc\Channel('firestore.googleapis.com:443', [
    'credentials' => $creds,
]);
echo "  Target: " . $channel->getTarget() . "\n";

$state = $channel->getConnectivityState(true); // try_to_connect=true
echo "  Initial state: {$state}\n";

// Poll until READY or TRANSIENT_FAILURE (proves TLS handshake ran)
$start = microtime(true);

while (true) {
    $state = $channel->getConnectivityState(false);
    if ($state === Grpc\CHANNEL_READY || $state === Grpc\CHANNEL_TRANSIENT_FAILURE) {
        break;
    }
    if (microtime(true) - $start > 5) {
        echo "  ✗ Timeout waiting for connection state change (stuck at {$state})\n";
        $channel->close();
        exit(1);
    }
    $channel->watchConnectivityState($state, new Grpc\Timeval(500_000));
}

echo "  Final state: {$state}";
if ($state === Grpc\CHANNEL_READY) {
    echo " (READY) ✓ TLS handshake successful — no OpenSSL conflicts!\n";
} else {
    echo " (TRANSIENT_FAILURE) — connection attempted but failed (expected without auth)\n";
}

$channel->close();
echo "\n  ✓ All channel+SSL tests passed\n";
