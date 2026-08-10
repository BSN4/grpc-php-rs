<?php
/**
 * google/cloud-spanner end-to-end against the official Spanner emulator.
 * The heaviest gRPC consumer in google-cloud-php: sessions, long-running
 * operations (instance/database creation), ExecuteStreamingSql (server
 * streaming), commits — through gax + optional grpc-gcp channel pooling.
 */
declare(strict_types=1);

require '/app/vendor/autoload.php';

use Google\Cloud\Spanner\SpannerClient;

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== google/cloud-spanner vs emulator (" . getenv('SPANNER_EMULATOR_HOST') . ") ===\n\n";

$spanner = new SpannerClient(['projectId' => 'test-project',
    'credentials' => new \Google\Auth\Credentials\InsecureCredentials()]);

// Retry until the emulator is ready
$ready = false;
$deadline = time() + 60;
while (time() < $deadline) {
    try { iterator_to_array($spanner->instanceConfigurations()); $ready = true; break; }
    catch (Throwable $e) { usleep(500_000); }
}
check('emulator reachable', $ready);
if (!$ready) { echo "\n=== {$passed}/{$tests} ===\n"; exit(1); }

$config = $spanner->instanceConfiguration('emulator-config');
$instance = $spanner->instance('test-instance');
if (!$instance->exists()) {
    $op = $instance->create($config, ['nodeCount' => 1]);
    $op->pollUntilComplete(['maxPollingDurationSeconds' => 60]);
}
check('instance created (LRO)', $instance->exists());

$db = $instance->database('testdb');
if (!$db->exists()) {
    $op = $db->create(['statements' => [
        'CREATE TABLE users (id INT64 NOT NULL, name STRING(64), blob BYTES(2048)) PRIMARY KEY (id)',
    ]]);
    $op->pollUntilComplete(['maxPollingDurationSeconds' => 60]);
}
check('database + DDL created (LRO)', $db->exists());

$bin = random_bytes(1024);
$db->insertOrUpdateBatch('users', [
    ['id' => 1, 'name' => 'alice', 'blob' => new \Google\Cloud\Spanner\Bytes($bin)],
    ['id' => 2, 'name' => 'bob', 'blob' => null],
]);
check('insert mutations committed', true);

$rows = iterator_to_array($db->execute('SELECT id, name, blob FROM users ORDER BY id'));
check('ExecuteStreamingSql rows', count($rows) === 2, 'got ' . count($rows));
check('row values intact', ($rows[0]['name'] ?? '') === 'alice' && ($rows[1]['name'] ?? '') === 'bob');
check('BYTES round-trip intact', isset($rows[0]['blob']) && (string)$rows[0]['blob']->get() === $bin);

$count = $db->runTransaction(function ($t) {
    $t->executeUpdate("UPDATE users SET name = 'carol' WHERE id = 2");
    $t->commit();
    return 1;
});
check('read-write transaction + DML', $count === 1);

$rows = iterator_to_array($db->execute('SELECT name FROM users WHERE id = 2'));
check('DML visible', ($rows[0]['name'] ?? '') === 'carol');

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
