<?php
/**
 * google/cloud-firestore end-to-end against the official Firestore emulator —
 * the real client stack (gax, CallCredentials plugin wiring, unary + commit
 * RPCs), unlike tests/test_firestore_fake.php which fakes the server side.
 */
declare(strict_types=1);

require '/app/vendor/autoload.php';

use Google\Cloud\Firestore\FirestoreClient;

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== google/cloud-firestore vs emulator (" . getenv('FIRESTORE_EMULATOR_HOST') . ") ===\n\n";

$fs = new FirestoreClient(['projectId' => 'test-project', 'transport' => 'grpc',
    'credentials' => new \Google\Auth\Credentials\InsecureCredentials()]);

$ready = false;
$deadline = time() + 60;
$doc = $fs->collection('parity')->document('probe');
while (time() < $deadline) {
    try { $doc->snapshot(); $ready = true; break; }
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

$doc->set(['name' => 'grpc-php-rs', 'count' => 42, 'bin' => new \Google\Cloud\Core\Blob("\x00\xff\x10"), 'nested' => ['a' => [1, 2, 3]]]);
check('document set', true);

$snap = $doc->snapshot();
check('snapshot exists', $snap->exists());
check('scalar fields intact', $snap['name'] === 'grpc-php-rs' && $snap['count'] === 42);
check('binary field intact', ($snap['bin'] ?? null) instanceof \Google\Cloud\Core\Blob
    && (string)$snap['bin'] === "\x00\xff\x10");
check('nested array intact', ($snap['nested'] ?? null) === ['a' => [1, 2, 3]]);

$doc->update([['path' => 'count', 'value' => 43]]);
check('update visible', $doc->snapshot()['count'] === 43);

$docs = iterator_to_array($fs->collection('parity')->where('count', '=', 43)->documents());
check('query returns doc', count($docs) === 1);

$doc->delete();
check('delete', !$doc->snapshot()->exists());

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
