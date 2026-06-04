<?php
/**
 * Regression test: a CallCredentials plugin that THROWS must surface a clean
 * catchable exception — not crash the process.
 *
 * Previously, invoke_call_plugin() Debug-formatted the ext-php-rs error,
 * which recursively walks the thrown PHP exception's object graph. Exception
 * graphs are cyclic, ext-php-rs's Debug has no cycle detection, so formatting
 * recursed until stack overflow (SIGSEGV). Found via FirestoreClient->set()
 * with unconfigured Google credentials (the auth closure throws).
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_plugin_exception.php
 */
declare(strict_types=1);

echo "=== CallCredentials Plugin Exception Test ===\n\n";

$tests = 0;
$passed = 0;
function check(string $name, bool $result, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

$target = 'localhost:50051';
$channel = new Grpc\Channel($target, []);

// Plugin that throws — mimics Google auth failing to fetch a token.
// The exception's trace references objects that reference the exception,
// producing the cyclic graph that triggered the infinite Debug recursion.
$creds = Grpc\CallCredentials::createFromPlugin(function (string $serviceUrl) {
    throw new RuntimeException('token fetch failed (no credentials)');
});

$call = new Grpc\Call($channel, '/grpc.testing.TestService/Echo', Grpc\Timeval::infFuture());
$call->setCredentials($creds);

$caught = null;
try {
    $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA  => [],
        Grpc\OP_SEND_MESSAGE           => "\x0a\x02hi",
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
        Grpc\OP_RECV_INITIAL_METADATA  => true,
        Grpc\OP_RECV_MESSAGE           => true,
        Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
    ]);
} catch (Throwable $e) {
    $caught = $e;
}

// The mere fact we reach here means no SIGSEGV — that's the regression.
check('process survived plugin exception (no SIGSEGV)', true);
check('exception surfaced to PHP', $caught !== null);
check('exception message names the plugin failure',
    $caught !== null && str_contains($caught->getMessage(), 'plugin threw'),
    $caught !== null ? "got: {$caught->getMessage()}" : 'no exception');
check('original message preserved',
    $caught !== null && str_contains($caught->getMessage(), 'token fetch failed'),
    $caught !== null ? "got: {$caught->getMessage()}" : 'no exception');

// Channel must remain usable after the failed call
$call2 = new Grpc\Call($channel, '/grpc.testing.TestService/Echo', Grpc\Timeval::infFuture());
$r = $call2->startBatch([
    Grpc\OP_SEND_INITIAL_METADATA  => [],
    Grpc\OP_SEND_MESSAGE           => "\x0a\x02hi",
    Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
    Grpc\OP_RECV_INITIAL_METADATA  => true,
    Grpc\OP_RECV_MESSAGE           => true,
    Grpc\OP_RECV_STATUS_ON_CLIENT  => true,
]);
check('channel still usable after plugin failure', ($r->status->code ?? -1) === Grpc\STATUS_OK);

$channel->close();

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
