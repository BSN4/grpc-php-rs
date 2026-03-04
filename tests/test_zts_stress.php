<?php
declare(strict_types=1);

// Serves as a FrankenPHP worker endpoint for ZTS concurrency stress testing.
// Each request creates a channel, attempts TLS, and closes it.
// Under the old C grpc extension this SIGSEGVs within seconds.

header('Content-Type: text/plain');

$id = bin2hex(random_bytes(4));
$start = hrtime(true);

try {
    $creds = Grpc\ChannelCredentials::createSsl();
    $channel = new Grpc\Channel('firestore.googleapis.com:443', [
        'credentials' => $creds,
    ]);

    // Force connection attempt (triggers TLS from background tokio threads)
    $channel->getConnectivityState(true);

    // Wait briefly for state change
    $state = $channel->getConnectivityState(false);
    $channel->watchConnectivityState($state, new Grpc\Timeval(1_000_000));
    $finalState = $channel->getConnectivityState(false);

    $channel->close();

    $elapsed = (hrtime(true) - $start) / 1_000_000;
    echo "OK id={$id} state={$finalState} elapsed={$elapsed}ms\n";

} catch (\Throwable $e) {
    $elapsed = (hrtime(true) - $start) / 1_000_000;
    echo "FAIL id={$id} error={$e->getMessage()} elapsed={$elapsed}ms\n";
    http_response_code(500);
}
