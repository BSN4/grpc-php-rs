<?php
/**
 * google/cloud-pubsub end-to-end against the official Pub/Sub emulator.
 * Exercises gax + grpc transport: topic/subscription admin (unary),
 * publish (unary with batching), pull + ack (unary with long deadlines).
 */
declare(strict_types=1);

require '/app/vendor/autoload.php';

use Google\Cloud\PubSub\PubSubClient;

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== google/cloud-pubsub vs emulator (" . getenv('PUBSUB_EMULATOR_HOST') . ") ===\n\n";

$pubsub = new PubSubClient(['projectId' => 'test-project', 'transport' => 'grpc',
    'credentials' => new \Google\Auth\Credentials\InsecureCredentials()]);

// Retry until the emulator is ready
$ready = false;
$deadline = time() + 60;
while (time() < $deadline) {
    try { iterator_to_array($pubsub->topics()); $ready = true; break; }
    catch (Throwable $e) { usleep(500_000); }
}
check('emulator reachable', $ready);
if (!$ready) { echo "\n=== {$passed}/{$tests} ===\n"; exit(1); }

$topic = $pubsub->createTopic('t-' . bin2hex(random_bytes(4)));
check('createTopic', $topic->exists());

$sub = $topic->subscribe('s-' . bin2hex(random_bytes(4)));
check('subscribe', $sub->exists());

$topic->publish(['data' => 'one', 'attributes' => ['k' => 'v1']]);
$topic->publishBatch([
    ['data' => 'two'],
    ['data' => str_repeat('x', 65536)], // 64KB payload through the stack
]);
check('publish x3', true);

$received = [];
$deadline = time() + 30;
while (count($received) < 3 && time() < $deadline) {
    foreach ($sub->pull(['maxMessages' => 10, 'returnImmediately' => true]) as $m) {
        $received[] = $m->data();
        $sub->acknowledge($m);
    }
    if (count($received) < 3) usleep(200_000);
}
sort($received);
check('pulled all 3 messages', count($received) === 3, 'got ' . count($received));
check('payloads intact incl. 64KB', in_array('one', $received, true)
    && in_array('two', $received, true)
    && in_array(str_repeat('x', 65536), $received, true));

$sub->delete();
$topic->delete();
check('cleanup delete', true);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
