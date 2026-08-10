<?php
/**
 * Promise-based async unary calls through the real grpc/grpc PHP library —
 * the exact layer google/shopping-merchant-products (and every google-cloud-php
 * client) sits on. Reproduces issue #18: gax's GrpcTransport starts N
 * UnaryCalls, wraps each wait() in a Guzzle promise, and the caller runs
 * Utils::settle($promises)->wait(). All N round trips must overlap.
 *
 * Requires: composer require grpc/grpc guzzlehttp/promises
 * Server:   tests/server (SlowEcho delays 250ms per call)
 */
declare(strict_types=1);

require getenv('COMPOSER_AUTOLOAD') ?: __DIR__ . '/../vendor/autoload.php';

use Grpc\Channel;
use Grpc\ChannelCredentials;
use Grpc\UnaryCall;
use GuzzleHttp\Promise\Promise;
use GuzzleHttp\Promise\Utils;

const SLOW_ECHO_DELAY_MS = 250; // matches SLOW_ECHO_DELAY in tests/server

class RawMsg {
    public string $wire = '';
    public function __construct(string $wire = '') { $this->wire = $wire; }
    public function serializeToString(): string { return $this->wire; }
    public function mergeFromString($data): void { $this->wire = (string)$data; }
}

function encode_varint(int $v): string {
    $b = '';
    while ($v > 0x7f) { $b .= chr(($v & 0x7f) | 0x80); $v >>= 7; }
    return $b . chr($v & 0x7f);
}
function encode_payload(string $d): string {
    return "\x0a" . encode_varint(strlen($d)) . $d;
}

// Exact promise shape from google/gax GrpcTransport::startUnaryCall
function gaxPromise(UnaryCall $call): Promise {
    $promise = new Promise(
        function () use ($call, &$promise) {
            [$response, $status] = $call->wait();
            if ($status->code !== 0) {
                $promise->reject(new RuntimeException($status->details, $status->code));
                return;
            }
            $promise->resolve($response);
        },
        [$call, 'cancel']
    );
    return $promise;
}

$tests = 0; $passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== Promise concurrency (grpc/grpc + guzzle, issue #18) ===\n\n";

$channel = new Channel('localhost:50051', ['credentials' => ChannelCredentials::createInsecure()]);
$n = 20;

$t0 = hrtime(true);
$promises = [];
for ($i = 0; $i < $n; $i++) {
    $call = new UnaryCall(
        $channel,
        '/grpc.testing.TestService/SlowEcho',
        [RawMsg::class, 'mergeFromString'],
        []
    );
    $call->start(new RawMsg(encode_payload("msg-{$i}")), [], []);
    $promises[] = gaxPromise($call);
}
$start_ms = (hrtime(true) - $t0) / 1e6;

$results = Utils::settle($promises)->wait();
$total_ms = (hrtime(true) - $t0) / 1e6;

$fulfilled = 0;
$intact = 0;
foreach ($results as $i => $r) {
    if ($r['state'] !== 'fulfilled') continue;
    $fulfilled++;
    if ($r['value'] instanceof RawMsg && $r['value']->wire === encode_payload("msg-{$i}")) $intact++;
}

printf("  %d calls, start()=%.0fms, settle()->wait()=%.0fms (serialised would be ~%dms)\n",
    $n, $start_ms, $total_ms, $n * SLOW_ECHO_DELAY_MS);

check("all {$n} promises fulfilled", $fulfilled === $n, "fulfilled={$fulfilled}");
check("all {$n} responses intact and in order", $intact === $n, "intact={$intact}");
check('start() phase does not block', $start_ms < 500, sprintf('%.0fms', $start_ms));
check(
    'settle wait ran concurrently',
    $total_ms < $n * SLOW_ECHO_DELAY_MS / 4,
    sprintf('%.0fms, limit %dms', $total_ms, $n * SLOW_ECHO_DELAY_MS / 4)
);

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
