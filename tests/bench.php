<?php
/**
 * Comparative benchmark — runs identically under grpc-php-rs and official
 * ext-grpc against the local test server (./test.sh bench diffs the two).
 *
 * Scenario matrix mirrors the known third-party comparisons: cold start,
 * warm unary, unary payload sizes, server-stream message counts and payload
 * sizes, and concurrent split-batch unary (the async gax pattern).
 *
 * Output: one line per scenario, "name|median_us|p95_us" (machine-parsed).
 */
declare(strict_types=1);

$TARGET = getenv('GRPC_TARGET') ?: 'localhost:50051';
$REVS = (int)(getenv('BENCH_REVS') ?: 50);

function ep(string $d): string {
    if ($d === '') return '';
    $len = strlen($d);
    $buf = '';
    while ($len > 0x7f) { $buf .= chr(($len & 0x7f) | 0x80); $len >>= 7; }
    return "\x0a" . $buf . chr($len & 0x7f) . $d;
}
function chan(): Grpc\Channel {
    global $TARGET;
    return new Grpc\Channel($TARGET, ['credentials' => Grpc\ChannelCredentials::createInsecure()]);
}
function unary(Grpc\Channel $ch, string $payload): void {
    $call = new Grpc\Call($ch, '/grpc.testing.TestService/Echo', Grpc\Timeval::infFuture());
    $r = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep($payload)],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    if ($r->status->code !== 0) { fwrite(STDERR, "unary failed: {$r->status->code}\n"); exit(1); }
}
function stream(Grpc\Channel $ch, string $payload, int $expect = -1): int {
    $call = new Grpc\Call($ch, '/grpc.testing.TestService/StreamEcho', Grpc\Timeval::infFuture());
    $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep($payload)],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
    ]);
    $n = 0;
    while (true) {
        $r = $call->startBatch([Grpc\OP_RECV_MESSAGE => true]);
        if ($r->message === null) break;
        $n++;
    }
    $s = $call->startBatch([Grpc\OP_RECV_STATUS_ON_CLIENT => true]);
    // A benchmark that measures a broken stream measures nothing: verify.
    if ($s->status->code !== 0 || ($expect >= 0 && $n !== $expect)) {
        fwrite(STDERR, "stream failed: code={$s->status->code} n={$n} expect={$expect}\n");
        exit(1);
    }
    return $n;
}
/** @param callable():void $fn */
function scenario(string $name, int $revs, callable $fn): void {
    $fn(); $fn(); // warmup
    $times = [];
    for ($i = 0; $i < $revs; $i++) {
        $t0 = hrtime(true);
        $fn();
        $times[] = (hrtime(true) - $t0) / 1000.0; // µs
    }
    sort($times);
    $median = $times[intdiv(count($times), 2)];
    $p95 = $times[(int)floor(count($times) * 0.95)];
    printf("%s|%.1f|%.1f\n", $name, $median, $p95);
}

$warm = chan();
unary($warm, 'warmup');

// ── Cold: channel construct + first unary ──
scenario('cold_channel_unary', min($REVS, 30), function () {
    $ch = chan();
    unary($ch, 'hi');
    $ch->close();
});

// ── Warm unary payload ladder ──
foreach ([0, 100, 1024, 10240, 102400] as $size) {
    $payload = $size === 0 ? '' : str_repeat('x', $size);
    scenario("unary_payload_{$size}", $REVS, function () use ($warm, $payload) {
        unary($warm, $payload);
    });
}

// ── Server streaming: message-count ladder (1-byte messages) ──
foreach ([10, 100, 1000] as $count) {
    scenario("stream_count_{$count}", max(10, intdiv($REVS, ($count >= 1000 ? 5 : 1))), function () use ($warm, $count) {
        stream($warm, "repeat:{$count}", $count);
    });
}

// ── Server streaming: payload ladder (3 echoed messages each) ──
foreach ([100, 1024, 10240] as $size) {
    scenario("stream_payload_{$size}", $REVS, function () use ($warm, $size) {
        stream($warm, str_repeat('y', $size), 3);
    });
}

// ── Concurrent split-batch unary (async gax pattern), 250ms server delay ──
scenario('async_50_slowecho', 5, function () use ($warm) {
    $calls = [];
    for ($i = 0; $i < 50; $i++) {
        $call = new Grpc\Call($warm, '/grpc.testing.TestService/SlowEcho', Grpc\Timeval::infFuture());
        $call->startBatch([
            Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('x')],
            Grpc\OP_SEND_CLOSE_FROM_CLIENT => true,
        ]);
        $calls[] = $call;
    }
    foreach ($calls as $call) {
        $r = $call->startBatch([
            Grpc\OP_RECV_INITIAL_METADATA => true, Grpc\OP_RECV_MESSAGE => true,
            Grpc\OP_RECV_STATUS_ON_CLIENT => true,
        ]);
        if ($r->status->code !== 0 || $r->message === null) {
            fwrite(STDERR, "async unary failed: {$r->status->code}\n");
            exit(1);
        }
    }
});
