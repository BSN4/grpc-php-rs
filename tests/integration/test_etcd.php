<?php
/**
 * aternos/etcd against a real etcd v3 server — non-Google ecosystem coverage.
 * etcd's KV API is plain gRPC (grpc/grpc lib + generated protos), so this
 * exercises the extension outside the gax stack entirely.
 */
declare(strict_types=1);

require '/etcd/vendor/autoload.php';

use Aternos\Etcd\Client;

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

$host = getenv('ETCD_HOST') ?: 'etcd:2379';
echo "=== aternos/etcd vs etcd v3 ({$host}) ===\n\n";

$client = new Client($host);

$ready = false;
$deadline = time() + 60;
while (time() < $deadline) {
    try { $client->get('probe'); $ready = true; break; }
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
check('etcd reachable', $ready);
if (!$ready) { echo "\n=== {$passed}/{$tests} ===\n"; exit(1); }

$client->put('parity/key1', 'hello');
check('put', true);
check('get round-trip', $client->get('parity/key1') === 'hello');

$bin = "\x00\xff\x10" . random_bytes(64);
$client->put('parity/bin', $bin);
check('binary value round-trip', $client->get('parity/bin') === $bin);

check('putIf (compare-and-swap)', $client->putIf('parity/key1', 'world', 'hello') === true);
check('swap visible', $client->get('parity/key1') === 'world');
check('putIf fails on stale compare', $client->putIf('parity/key1', 'nope', 'hello') !== true);

$lease = $client->getLeaseID(60);
$client->put('parity/leased', 'ttl', false, $lease);
check('lease + leased put', $client->get('parity/leased') === 'ttl');
$client->revokeLeaseID($lease);
check('lease revoked removes key', $client->get('parity/leased') === false);

$client->delete('parity/key1');
$client->delete('parity/bin');
check('delete', $client->get('parity/key1') === false);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
