<?php
/**
 * ext-grpc parity ledger — every known behavioral divergence from the official
 * C extension, each proven by running the same code under both extensions
 * (see git history / audit of 2026-08-11 against pecl grpc 1.83.0).
 *
 * Each case declares:
 *   expected — what official ext-grpc observably does (the target)
 *   current  — what we do today (the documented divergence)
 *   fixed    — false while the divergence exists; flip to true when fixing it
 *
 * Semantics (strict-xfail):
 *   fixed=true  → observed must equal expected (regression check)
 *   fixed=false → observed must equal current; if it equals expected instead,
 *                 this test FAILS telling you to flip fixed=true, so the
 *                 ledger can never silently rot.
 *
 * All tests pass ⇔ every fixed case matches ext-grpc AND every open case
 * still behaves exactly as documented.
 *
 * Run:
 *   # Terminal 1: cargo run --manifest-path tests/server/Cargo.toml
 *   # Terminal 2: php tests/test_ext_grpc_parity.php
 */
declare(strict_types=1);

$TARGET = getenv('GRPC_TARGET') ?: 'localhost:50051';
const ECHO_M = '/grpc.testing.TestService/Echo';

function ep(string $d): string { return "\x0a" . chr(strlen($d)) . $d; }
function chan(): Grpc\Channel {
    global $TARGET;
    return new Grpc\Channel($TARGET, ['credentials' => Grpc\ChannelCredentials::createInsecure()]);
}
function dl3(): Grpc\Timeval { return Grpc\Timeval::now()->add(new Grpc\Timeval(3_000_000)); }
function fullBatch(): array {
    return [Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true, Grpc\OP_RECV_INITIAL_METADATA=>true,
        Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true];
}
/** Run $fn, normalizing exceptions to their class name. */
function obs(callable $fn): string {
    try { return (string)$fn(); }
    catch (Throwable $e) { return get_class($e); }
}

$cases = [];
function ledger(string $id, bool $fixed, string $expected, string $current, callable $fn): void {
    global $cases;
    $cases[] = compact('id', 'fixed', 'expected', 'current', 'fn');
}

// ═════════════════════════ Call / startBatch layer ═════════════════════════

ledger('send-op-result-booleans', false, 'md=1 msg=1 close=1', 'md= msg= close=', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $r = $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $out = sprintf('md=%s msg=%s close=%s', ($r->send_metadata ?? ''), ($r->send_message ?? ''), ($r->send_close ?? ''));
    $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    return $out;
});

ledger('success-trailers-clean-split', false, 'keys=', 'keys=grpc-status', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $s = $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    $k = array_keys((array)$s->status->metadata);
    sort($k);
    return 'keys=' . implode(',', $k);
});

ledger('recv-after-done-errors', false, 'LogicException', 'code=0', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch(fullBatch());
    return obs(function () use ($call) {
        $s = $call->startBatch([Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
        return 'code=' . $s->status->code;
    });
});

ledger('initial-md-transport-headers-stripped', false, 'ct=0 gs=0', 'ct=1 gs=1', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $r = $call->startBatch(fullBatch());
    $md = (array)$r->metadata;
    return sprintf('ct=%d gs=%d', (int)array_key_exists('content-type', $md), (int)array_key_exists('grpc-status', $md));
});

ledger('unknown-op-rejected', false, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([999 => true]); return 'accepted'; });
});

ledger('bad-metadata-uppercase-key', false, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => ['UPPER' => ['v']]]); return 'accepted'; });
});

ledger('bad-metadata-plain-string-value', false, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => ['k' => 'plain']]); return 'accepted'; });
});

ledger('bad-metadata-int-key', false, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => [1 => ['v']]]); return 'accepted'; });
});

ledger('send-message-bare-string-rejected', false, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) {
        $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>ep('hi'),
            Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
        return 'accepted';
    });
});

ledger('send-message-non-string-rejected', false, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_MESSAGE => ['message' => 0]]); return 'accepted'; });
});

// Intentional divergence: ext-grpc 1.83 HANGS FOREVER on a non-int flags value
// (reproduced 2026-08-11; C source claims InvalidArgumentException). We accept
// and ignore the flag. Never emulate the hang; validating would also be fine.
ledger('flags-bad-type-no-hang', true, 'accepted', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) {
        $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi'), 'flags'=>'bad'],
            Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
        return 'accepted';
    });
});

ledger('closed-channel-batch-rejected', false, 'RuntimeException', 'code=0', function () {
    $c = chan();
    $call = new Grpc\Call($c, ECHO_M, dl3());
    $c->close();
    return obs(function () use ($call) { $r = $call->startBatch(fullBatch()); return 'code=' . $r->status->code; });
});

ledger('ctor-closed-channel-exception-class', false, 'InvalidArgumentException', 'Exception', function () {
    $c = chan();
    $c->close();
    return obs(function () use ($c) { new Grpc\Call($c, ECHO_M, dl3()); return 'accepted'; });
});

ledger('getpeer-transport-format', false, 'transport-peer', 'target-string', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch(fullBatch());
    $p = $call->getPeer();
    return (str_starts_with($p, 'ipv4:') || str_starts_with($p, 'ipv6:')) ? 'transport-peer' : 'target-string';
});

ledger('host-override-not-in-getpeer', false, 'peer', 'override-leaked', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3(), 'override.example.com');
    $call->startBatch(fullBatch());
    return $call->getPeer() === 'override.example.com' ? 'override-leaked' : 'peer';
});

ledger('double-send-initial-md-rejected', false, 'LogicException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => []]);
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => ['k'=>['v']]]); return 'accepted'; });
});

ledger('call-channel-property', false, 'true', 'false', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return var_export(property_exists($call, 'channel'), true);
});

ledger('status-property-order', false, 'metadata,code,details', 'code,details,metadata', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $r = $call->startBatch(fullBatch());
    return implode(',', array_keys((array)$r->status));
});

// ══════════════════ Channel / Credentials / Timeval layer ══════════════════

ledger('connectivity-state-transitions', false, 'state=2', 'state=0', function () {
    $ch = chan();
    $ch->getConnectivityState(true);
    usleep(500_000);
    return 'state=' . $ch->getConnectivityState();
});

ledger('watch-connectivity-honors-deadline', false, 'deadline-false', 'immediate-true', function () {
    $ch = chan();
    $cur = $ch->getConnectivityState();
    $t0 = microtime(true);
    $r = $ch->watchConnectivityState($cur, Grpc\Timeval::now()->add(new Grpc\Timeval(300_000)));
    $elapsed = microtime(true) - $t0;
    if (!$r && $elapsed >= 0.25) return 'deadline-false';
    if ($r && $elapsed < 0.05) return 'immediate-true';
    return sprintf('other(r=%s,%.2fs)', var_export($r, true), $elapsed);
});

ledger('invalid-credentials-rejected', false, 'InvalidArgumentException', 'constructed', function () {
    global $TARGET;
    return obs(function () use ($TARGET) {
        new Grpc\Channel($TARGET, ['credentials' => new Grpc\Timeval(100)]);
        return 'constructed';
    });
});

ledger('persistent-channel-shared', false, 'a=2 b=2', 'a=0 b=0', function () {
    $a = chan();
    $b = chan();
    $a->getConnectivityState(true);
    usleep(400_000);
    return sprintf('a=%d b=%d', $a->getConnectivityState(), $b->getConnectivityState());
});

ledger('channel-args-bool-rejected', false, 'InvalidArgumentException', 'accepted', function () {
    global $TARGET;
    return obs(function () use ($TARGET) {
        new Grpc\Channel($TARGET, ['credentials' => null, 'grpc.enable_retries' => true]);
        return 'accepted';
    });
});

ledger('call-error-constants', false, 'if=9 tmo=1 aa=0', 'if=8 tmo=0 aa=1', function () {
    return sprintf('if=%s tmo=%d aa=%d',
        defined('Grpc\CALL_ERROR_INVALID_FLAGS') ? Grpc\CALL_ERROR_INVALID_FLAGS : '-',
        (int)defined('Grpc\CALL_ERROR_TOO_MANY_OPERATIONS'),
        (int)defined('Grpc\CALL_ERROR_ALREADY_ACCEPTED'));
});

ledger('closed-channel-exception-class', false, 'RuntimeException', 'Exception', function () {
    $c = chan();
    $c->close();
    return obs(function () use ($c) { $c->getConnectivityState(); return 'ok'; });
});

ledger('connectivity-state-int-arg-rejected', false, 'InvalidArgumentException', 'accepted', function () {
    $c = chan();
    return obs(function () use ($c) { $c->getConnectivityState(123); return 'accepted'; });
});

ledger('timeval-float-ctor-coerced', false, 'ok', 'Exception', function () {
    return obs(function () { new Grpc\Timeval(2.5e6); return 'ok'; });
});

ledger('timeval-sleepuntil-exists', false, 'true', 'false', function () {
    return var_export(method_exists('Grpc\Timeval', 'sleepUntil'), true);
});

ledger('timeval-infinity-saturation', false, 'compare=0', 'compare=1', function () {
    $inf = Grpc\Timeval::infFuture();
    return 'compare=' . Grpc\Timeval::compare($inf, $inf->subtract(new Grpc\Timeval(1000)));
});

ledger('channelcredentials-createxds-exists', false, 'true', 'false', function () {
    return var_export(method_exists('Grpc\ChannelCredentials', 'createXds'), true);
});

ledger('server-class-exists', false, 'true', 'false', function () {
    return var_export(class_exists('Grpc\Server'), true);
});

ledger('fork-support-ini-registered', false, 'string', 'false', function () {
    $v = ini_get('grpc.enable_fork_support');
    return $v === false ? 'false' : 'string';
});

ledger('callcreds-plugin-not-invoked-on-plaintext', false, 'not-invoked', 'invoked-with-string', function () {
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/EmptyResponse', dl3());
    $seen = 'not-invoked';
    $call->setCredentials(Grpc\CallCredentials::createFromPlugin(function ($c) use (&$seen) {
        $seen = is_object($c) ? 'invoked-with-object' : 'invoked-with-' . gettype($c);
        return [];
    }));
    try {
        $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('x')],
            Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
            Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true]);
    } catch (Throwable $e) { /* result via $seen */ }
    return $seen;
});

ledger('timeval-methods-no-return-types', false, 'false', 'true', function () {
    return var_export((new ReflectionMethod('Grpc\Timeval', 'add'))->hasReturnType(), true);
});

// ═══════════════════════════════ Runner ════════════════════════════════════

$pass = 0;
$fail = 0;
$open = 0;
foreach ($cases as $c) {
    try { $observed = ($c['fn'])(); }
    catch (Throwable $e) { $observed = 'HARNESS-ERROR: ' . get_class($e) . ': ' . $e->getMessage(); }

    if ($c['fixed']) {
        if ($observed === $c['expected']) { $pass++; echo "  ✓ {$c['id']} (parity)\n"; }
        else { $fail++; echo "  ✗ {$c['id']}: REGRESSED — expected \"{$c['expected']}\", got \"{$observed}\"\n"; }
    } else {
        if ($observed === $c['current']) { $pass++; $open++; echo "  ✓ {$c['id']} (documented divergence, ext-grpc: \"{$c['expected']}\")\n"; }
        elseif ($observed === $c['expected']) { $fail++; echo "  ✗ {$c['id']}: NOW MATCHES ext-grpc — flip fixed=true in this file\n"; }
        else { $fail++; echo "  ✗ {$c['id']}: drifted — documented \"{$c['current']}\", got \"{$observed}\"\n"; }
    }
}

printf("\n=== %d/%d passed (%d at parity, %d documented divergences open) ===\n",
    $pass, count($cases), $pass - $open, $open);
exit($fail === 0 ? 0 : 1);
