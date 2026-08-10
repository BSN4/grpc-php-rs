<?php
/**
 * google/cloud-datastore end-to-end against the official Datastore emulator
 * over the gRPC transport: entity upsert/lookup, queries, transactions.
 */
declare(strict_types=1);

require '/app/vendor/autoload.php';

use Google\Cloud\Datastore\DatastoreClient;

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== google/cloud-datastore vs emulator (" . getenv('DATASTORE_EMULATOR_HOST') . ") ===\n\n";

$ds = new DatastoreClient(['projectId' => 'test-project', 'transport' => 'grpc',
    'credentials' => new \Google\Auth\Credentials\InsecureCredentials()]);

$key = $ds->key('Task', 'task-1');

$ready = false;
$deadline = time() + 60;
while (time() < $deadline) {
    try { $ds->lookup($key); $ready = true; break; }
    catch (Throwable $e) {
        // Retry connection-shaped errors only; anything else is a real bug and must fail now
        $m = $e->getMessage();
        if (stripos($m, 'unavailable') === false && stripos($m, 'connect') === false
            && stripos($m, 'refused') === false && stripos($m, 'deadline') === false) {
            throw $e;
        }
        usleep(500_000);
    }
}
check('emulator reachable', $ready);
if (!$ready) { echo "\n=== {$passed}/{$tests} ===\n"; exit(1); }

$ds->upsert($ds->entity($key, ['name' => 'write tests', 'done' => false, 'priority' => 7]));
check('entity upsert', true);

$e = $ds->lookup($key);
check('lookup intact', $e !== null && $e['name'] === 'write tests' && $e['priority'] === 7);

$ds->upsertBatch([
    $ds->entity($ds->key('Task', 'task-2'), ['name' => 'second', 'done' => true, 'priority' => 1]),
    $ds->entity($ds->key('Task', 'task-3'), ['name' => 'third', 'done' => false, 'priority' => 9]),
]);
$q = $ds->query()->kind('Task')->filter('done', '=', false)->order('priority');
$names = [];
foreach ($ds->runQuery($q) as $entity) {
    $names[] = $entity['name'];
}
check('query + filter + order', $names === ['write tests', 'third'], implode(',', $names));

$tx = $ds->transaction();
$t = $tx->lookup($key);
$t['priority'] = 8;
$tx->upsert($t);
$tx->commit();
check('transaction commit', $ds->lookup($key)['priority'] === 8);

$ds->deleteBatch([$key, $ds->key('Task', 'task-2'), $ds->key('Task', 'task-3')]);
check('cleanup delete', $ds->lookup($key) === null);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
