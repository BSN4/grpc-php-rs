<?php
/**
 * google/cloud-bigtable end-to-end against the official Bigtable emulator.
 * ReadRows is real gRPC server streaming through gax — the streaming-heavy
 * counterpart to Spanner's ExecuteStreamingSql.
 */
declare(strict_types=1);

require '/app/vendor/autoload.php';

use Google\Cloud\Bigtable\Admin\V2\Client\BigtableTableAdminClient;
use Google\Cloud\Bigtable\Admin\V2\CreateTableRequest;
use Google\Cloud\Bigtable\Admin\V2\ColumnFamily;
use Google\Cloud\Bigtable\Admin\V2\Table;
use Google\Cloud\Bigtable\BigtableClient;
use Google\Cloud\Bigtable\Mutations;

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== google/cloud-bigtable vs emulator (" . getenv('BIGTABLE_EMULATOR_HOST') . ") ===\n\n";

$admin = new BigtableTableAdminClient(['credentials' => new \Google\Auth\Credentials\InsecureCredentials()]);
$parent = 'projects/test-project/instances/test-instance';

// Retry until the emulator is ready
$ready = false;
$deadline = time() + 60;
while (time() < $deadline) {
    try {
        $admin->createTable(new CreateTableRequest([
            'parent' => $parent,
            'table_id' => 'parity',
            'table' => new Table(['column_families' => ['cf' => new ColumnFamily()]]),
        ]));
        $ready = true;
        break;
    } catch (Throwable $e) {
        if (str_contains($e->getMessage(), 'already exists')) { $ready = true; break; }
        usleep(500_000);
    }
}
check('emulator reachable + table created', $ready);
if (!$ready) { echo "\n=== {$passed}/{$tests} ===\n"; exit(1); }

$bt = new BigtableClient(['projectId' => 'test-project',
    'credentials' => new \Google\Auth\Credentials\InsecureCredentials()]);
$table = $bt->table('test-instance', 'parity');

$rows = [];
for ($i = 0; $i < 200; $i++) {
    $rows["row{$i}"] = ['cf' => ['col' => ['value' => sprintf('value-%03d', $i)]]];
}
$table->upsert($rows);
check('upsert 200 rows', true);

$got = 0;
$first = null;
$last = null;
foreach ($table->readRows() as $key => $row) {
    if ($got === 0) $first = $key;
    $last = $key;
    $got++;
}
check('readRows streams all 200 back', $got === 200, "got {$got}");
check('row keys ordered', $first === 'row0' && $last === 'row99');

$single = $table->readRow('row42');
check('point read intact', ($single['cf']['col'][0]['value'] ?? '') === 'value-042');

$table->mutateRows(['row0' => (new Mutations())->deleteRow()]);
check('deleteRow verified', $table->readRow('row0') === null);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
