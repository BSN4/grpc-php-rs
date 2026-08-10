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

ledger('send-op-result-booleans', true, 'md=1 msg=1 close=1', 'md= msg= close=', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $r = $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $out = sprintf('md=%s msg=%s close=%s', ($r->send_metadata ?? ''), ($r->send_message ?? ''), ($r->send_close ?? ''));
    $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    return $out;
});

ledger('success-trailers-clean-split', true, 'keys=', 'keys=grpc-status', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $s = $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    $k = array_keys((array)$s->status->metadata);
    sort($k);
    return 'keys=' . implode(',', $k);
});

ledger('recv-after-done-errors', true, 'LogicException', 'code=0', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch(fullBatch());
    return obs(function () use ($call) {
        $s = $call->startBatch([Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
        return 'code=' . $s->status->code;
    });
});

ledger('initial-md-transport-headers-stripped', true, 'ct=0 gs=0', 'ct=1 gs=1', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $r = $call->startBatch(fullBatch());
    $md = (array)$r->metadata;
    return sprintf('ct=%d gs=%d', (int)array_key_exists('content-type', $md), (int)array_key_exists('grpc-status', $md));
});

ledger('unknown-op-rejected', true, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([999 => true]); return 'accepted'; });
});

ledger('bad-metadata-uppercase-key', true, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => ['UPPER' => ['v']]]); return 'accepted'; });
});

ledger('bad-metadata-plain-string-value', true, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => ['k' => 'plain']]); return 'accepted'; });
});

ledger('bad-metadata-int-key', true, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => [1 => ['v']]]); return 'accepted'; });
});

ledger('send-message-bare-string-rejected', true, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) {
        $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>ep('hi'),
            Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
        return 'accepted';
    });
});

ledger('send-message-non-string-rejected', true, 'InvalidArgumentException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_MESSAGE => ['message' => 0]]); return 'accepted'; });
});

// Intentional divergence: ext-grpc 1.83 HANGS FOREVER on a non-int flags
// value (reproduced 2026-08-11; its C source intends
// InvalidArgumentException). We throw the documented exception instead.
// Never emulate the hang.
ledger('flags-bad-type-no-hang', true, 'InvalidArgumentException', 'InvalidArgumentException', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return obs(function () use ($call) {
        $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('hi'), 'flags'=>'bad'],
            Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
        return 'accepted';
    });
});

ledger('closed-channel-batch-rejected', true, 'RuntimeException', 'code=0', function () {
    $c = chan();
    $call = new Grpc\Call($c, ECHO_M, dl3());
    $c->close();
    return obs(function () use ($call) { $r = $call->startBatch(fullBatch()); return 'code=' . $r->status->code; });
});

ledger('ctor-closed-channel-exception-class', true, 'InvalidArgumentException', 'Exception', function () {
    $c = chan();
    $c->close();
    return obs(function () use ($c) { new Grpc\Call($c, ECHO_M, dl3()); return 'accepted'; });
});

ledger('getpeer-transport-format', true, 'transport-peer', 'target-string', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch(fullBatch());
    $p = $call->getPeer();
    return (str_starts_with($p, 'ipv4:') || str_starts_with($p, 'ipv6:')) ? 'transport-peer' : 'target-string';
});

ledger('host-override-not-in-getpeer', true, 'peer', 'override-leaked', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3(), 'override.example.com');
    $call->startBatch(fullBatch());
    return $call->getPeer() === 'override.example.com' ? 'override-leaked' : 'peer';
});

ledger('double-send-initial-md-rejected', true, 'LogicException', 'accepted', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => []]);
    return obs(function () use ($call) { $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA => ['k'=>['v']]]); return 'accepted'; });
});

ledger('call-channel-property', false, 'true', 'false', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    return var_export(property_exists($call, 'channel'), true);
});

ledger('status-property-order', true, 'metadata,code,details', 'code,details,metadata', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $r = $call->startBatch(fullBatch());
    return implode(',', array_keys((array)$r->status));
});

// ══════════════════ Channel / Credentials / Timeval layer ══════════════════

ledger('connectivity-state-transitions', true, 'state=2', 'state=0', function () {
    $ch = chan();
    $ch->getConnectivityState(true);
    usleep(500_000);
    return 'state=' . $ch->getConnectivityState();
});

ledger('watch-connectivity-honors-deadline', true, 'deadline-false', 'immediate-true', function () {
    $ch = chan();
    $cur = $ch->getConnectivityState();
    $t0 = microtime(true);
    $r = $ch->watchConnectivityState($cur, Grpc\Timeval::now()->add(new Grpc\Timeval(300_000)));
    $elapsed = microtime(true) - $t0;
    if (!$r && $elapsed >= 0.25) return 'deadline-false';
    if ($r && $elapsed < 0.05) return 'immediate-true';
    return sprintf('other(r=%s,%.2fs)', var_export($r, true), $elapsed);
});

ledger('invalid-credentials-rejected', true, 'InvalidArgumentException', 'constructed', function () {
    global $TARGET;
    return obs(function () use ($TARGET) {
        new Grpc\Channel($TARGET, ['credentials' => new Grpc\Timeval(100)]);
        return 'constructed';
    });
});

ledger('persistent-channel-shared', true, 'a=2 b=2', 'a=0 b=0', function () {
    $a = chan();
    $b = chan();
    $a->getConnectivityState(true);
    usleep(400_000);
    return sprintf('a=%d b=%d', $a->getConnectivityState(), $b->getConnectivityState());
});

ledger('channel-args-bool-rejected', true, 'InvalidArgumentException', 'accepted', function () {
    global $TARGET;
    return obs(function () use ($TARGET) {
        new Grpc\Channel($TARGET, ['credentials' => null, 'grpc.enable_retries' => true]);
        return 'accepted';
    });
});

ledger('call-error-constants', true, 'if=9 tmo=1 aa=0', 'if=8 tmo=0 aa=1', function () {
    return sprintf('if=%s tmo=%d aa=%d',
        defined('Grpc\CALL_ERROR_INVALID_FLAGS') ? Grpc\CALL_ERROR_INVALID_FLAGS : '-',
        (int)defined('Grpc\CALL_ERROR_TOO_MANY_OPERATIONS'),
        (int)defined('Grpc\CALL_ERROR_ALREADY_ACCEPTED'));
});

ledger('closed-channel-exception-class', true, 'RuntimeException', 'Exception', function () {
    $c = chan();
    $c->close();
    return obs(function () use ($c) { $c->getConnectivityState(); return 'ok'; });
});

ledger('connectivity-state-int-arg-rejected', true, 'InvalidArgumentException', 'accepted', function () {
    $c = chan();
    return obs(function () use ($c) { $c->getConnectivityState(123); return 'accepted'; });
});

ledger('timeval-float-ctor-coerced', true, 'ok', 'Exception', function () {
    return obs(function () { new Grpc\Timeval(2.5e6); return 'ok'; });
});

ledger('timeval-sleepuntil-exists', true, 'true', 'false', function () {
    return var_export(method_exists('Grpc\Timeval', 'sleepUntil'), true);
});

ledger('timeval-infinity-saturation', true, 'compare=0', 'compare=1', function () {
    $inf = Grpc\Timeval::infFuture();
    return 'compare=' . Grpc\Timeval::compare($inf, $inf->subtract(new Grpc\Timeval(1000)));
});

ledger('channelcredentials-createxds-exists', false, 'true', 'false', function () {
    return var_export(method_exists('Grpc\ChannelCredentials', 'createXds'), true);
});

ledger('server-class-exists', false, 'true', 'false', function () {
    return var_export(class_exists('Grpc\Server'), true);
});

ledger('fork-support-ini-registered', true, 'string', 'false', function () {
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

ledger('timeval-methods-no-return-types', true, 'false', 'true', function () {
    return var_export((new ReflectionMethod('Grpc\Timeval', 'add'))->hasReturnType(), true);
});

// ═════════════ Wire-level cases (metadata-echo + TLS harness) ══════════════
// Proven 2026-08-12 via EchoMetadata (echoes request metadata + :authority as
// "key:hex,hex" lines) and the TLS listener on :50052 (CA: tests/server/tls).

$TLS_TARGET = str_replace('50051', '50052', $TARGET);
$CA_PEM = file_get_contents(__DIR__ . '/server/tls/ca.crt') ?: '';

function decode_body(?string $w): string {
    if ($w === null || $w === '') return '';
    $len = 0; $shift = 0; $i = 1;
    while (true) { $b = ord($w[$i]); $len |= ($b & 0x7f) << $shift; $i++; if (!($b & 0x80)) break; $shift += 7; }
    return substr($w, $i, $len);
}

/** Run EchoMetadata with $md; return "key:hex" lines for the given prefixes. */
function md_echo(Grpc\Channel $ch, array $md, array $prefixes, ?Grpc\CallCredentials $cc = null): string {
    $call = new Grpc\Call($ch, '/grpc.testing.TestService/EchoMetadata', dl3());
    if ($cc !== null) $call->setCredentials($cc);
    $r = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => $md, Grpc\OP_SEND_MESSAGE => ['message' => ep('x')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    if ($r->status->code !== 0) return "status={$r->status->code}";
    $keep = array_filter(explode("\n", decode_body($r->message)),
        fn($l) => array_any($prefixes, fn($p) => str_starts_with($l, $p)));
    sort($keep);
    return implode(' | ', $keep) ?: '(none)';
}
function tls_chan(): Grpc\Channel {
    global $TLS_TARGET, $CA_PEM;
    return new Grpc\Channel($TLS_TARGET, ['credentials' => Grpc\ChannelCredentials::createSsl($CA_PEM)]);
}

ledger('send-metadata-multi-value', false, 'x-multi:7631,7632', 'x-multi:7632', function () {
    return md_echo(chan(), ['x-multi' => ['v1', 'v2']], ['x-multi']);
});

ledger('send-bin-metadata', false, 'x-probe-bin:00ff10', '(none)', function () {
    return md_echo(chan(), ['x-probe-bin' => ["\x00\xff\x10"]], ['x-probe-bin']);
});

ledger('channel-default-authority', false, 'x-seen-authority:custom.example.com', 'x-seen-authority:(target)', function () {
    global $TARGET;
    $ch = new Grpc\Channel($TARGET, [
        'credentials' => Grpc\ChannelCredentials::createInsecure(),
        'grpc.default_authority' => 'custom.example.com',
    ]);
    $line = md_echo($ch, [], ['x-seen-authority']);
    $auth = hex2bin(explode(':', $line, 2)[1] ?? '') ?: '?';
    return 'x-seen-authority:' . ($auth === $TARGET ? '(target)' : $auth);
});

ledger('host-override-sets-authority', false, 'x-seen-authority:override.example.com', 'x-seen-authority:(target)', function () {
    global $TARGET;
    $ch = chan();
    $call = new Grpc\Call($ch, '/grpc.testing.TestService/EchoMetadata', dl3(), 'override.example.com');
    $r = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('x')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    foreach (explode("\n", decode_body($r->message)) as $l) {
        if (str_starts_with($l, 'x-seen-authority')) {
            $auth = hex2bin(explode(':', $l, 2)[1]) ?: '?';
            return 'x-seen-authority:' . ($auth === $TARGET ? '(target)' : $auth);
        }
    }
    return '(none)';
});

ledger('callcreds-plugin-context-object', false, 'object;keys=service_url+method_name', 'string', function () {
    $cc = Grpc\CallCredentials::createFromPlugin(function ($ctx) {
        $desc = is_object($ctx)
            ? 'object;keys=' . implode('+', array_keys((array)$ctx))
            : gettype($ctx);
        return ['x-parg' => [$desc]];
    });
    $line = md_echo(tls_chan(), [], ['x-parg'], $cc);
    if (!str_starts_with($line, 'x-parg:')) return $line;
    return (string)hex2bin(explode(',', explode(':', $line, 2)[1])[0]);
});

// ext-grpc 1.83's own composite is broken (observed: second plugin's metadata
// sent twice, first dropped). The target here is the documented contract —
// both plugins contribute — which neither implementation meets today.
ledger('callcreds-composite-both-plugins', false, 'x-p1:6f6e65 | x-p2:74776f', 'x-p1:6f6e65', function () {
    $cc1 = Grpc\CallCredentials::createFromPlugin(fn($c) => ['x-p1' => ['one']]);
    $cc2 = Grpc\CallCredentials::createFromPlugin(fn($c) => ['x-p2' => ['two']]);
    return md_echo(tls_chan(), [], ['x-p'], Grpc\CallCredentials::createComposite($cc1, $cc2));
});

ledger('callcreds-plugin-bad-return-fails', false, 'code=14', 'code=0', function () {
    $bad = Grpc\CallCredentials::createFromPlugin(fn($c) => 'not an array');
    $call = new Grpc\Call(tls_chan(), '/grpc.testing.TestService/Echo', dl3());
    $call->setCredentials($bad);
    $r = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('x')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    return 'code=' . $r->status->code;
});

// Intentional divergence: setDefaultRootsPem set BEFORE createSsl() works
// here (code=0); observed ext-grpc 1.83 fails the handshake (code=14) even in
// this order. Our behavior is the one google-cloud's BaseStub relies on.
ledger('roots-pem-late-creds-work', true, 'code=0', 'code=0', function () {
    global $CA_PEM;
    Grpc\ChannelCredentials::setDefaultRootsPem($CA_PEM);
    $late = Grpc\ChannelCredentials::createSsl();
    global $TLS_TARGET;
    $ch = new Grpc\Channel($TLS_TARGET, ['credentials' => $late]);
    $call = new Grpc\Call($ch, '/grpc.testing.TestService/Echo', dl3());
    $r = $call->startBatch([
        Grpc\OP_SEND_INITIAL_METADATA => [], Grpc\OP_SEND_MESSAGE => ['message' => ep('x')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT => true, Grpc\OP_RECV_INITIAL_METADATA => true,
        Grpc\OP_RECV_MESSAGE => true, Grpc\OP_RECV_STATUS_ON_CLIENT => true,
    ]);
    return 'code=' . $r->status->code;
});


// ═════════ Code-review findings, each verified by dual-run (2026-08-12) ═════
// Refuted and NOT listed: post-status RECV_MESSAGE returning null (ext-grpc
// blocks forever there; our LogicException is defensible), and
// watchConnectivityState(infFuture) "hanging" (ext-grpc blocks identically).

ledger('stream-status-midstream-no-deadlock', true, 'code=0 msg=present', 'code=0 msg=present', function () {
    // Asking for a message + final status mid-stream must not deadlock when
    // the server has more than STREAM_MSG_BUFFER messages queued.
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/StreamEcho', dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[], Grpc\OP_SEND_MESSAGE=>['message'=>ep('repeat:100')],
        Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $r = $call->startBatch([Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    return sprintf('code=%d msg=%s', $r->status->code, $r->message === null ? 'null' : 'present');
});

ledger('send-metadata-with-message-same-batch', false, 'x-f2a:76', '(none)', function () {
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/EchoMetadata', dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>['x-f2a'=>['v']], Grpc\OP_SEND_MESSAGE=>['message'=>ep('x')]]);
    $call->startBatch([Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $r = $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    if ($r->status->code !== 0) return "status={$r->status->code}";
    $k = array_filter(explode("\n", decode_body($r->message)), fn($l) => str_starts_with($l, 'x-f2a'));
    return implode(' | ', $k) ?: '(none)';
});

ledger('send-metadata-then-message-batch', false, 'x-f2b:76', '(none)', function () {
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/EchoMetadata', dl3());
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>['x-f2b'=>['v']]]);
    $call->startBatch([Grpc\OP_SEND_MESSAGE=>['message'=>ep('x')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $r = $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    if ($r->status->code !== 0) return "status={$r->status->code}";
    $k = array_filter(explode("\n", decode_body($r->message)), fn($l) => str_starts_with($l, 'x-f2b'));
    return implode(' | ', $k) ?: '(none)';
});

// ext-grpc rejects the illegal value with status 13 rather than corrupting it;
// we lossy-decode non-UTF-8 bytes to U+FFFD and silently drop control bytes.
ledger('send-metadata-raw-bytes-not-corrupted', false, 'status=13', 'x-f3raw:6869efbfbdefbfbd', function () {
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/EchoMetadata', dl3());
    try {
        $r = $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>['x-f3raw'=>["hi\xff\xfe"], 'x-f3ctl'=>["tok\nnl"]],
            Grpc\OP_SEND_MESSAGE=>['message'=>ep('x')], Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true,
            Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    } catch (Throwable $e) { return get_class($e); }
    if ($r->status->code !== 0) return "status={$r->status->code}";
    $k = array_filter(explode("\n", decode_body($r->message)), fn($l) => str_starts_with($l, 'x-f3'));
    sort($k);
    return implode(' | ', $k) ?: '(none)';
});

ledger('infpast-deadline-fails-immediately', false, 'code=4', 'code=0', function () {
    $call = new Grpc\Call(chan(), ECHO_M, Grpc\Timeval::infPast());
    $r = $call->startBatch(fullBatch());
    return 'code=' . $r->status->code;
});

ledger('timeval-zero-is-absolute', false, 'code=4', 'code=0', function () {
    $d = Grpc\Timeval::zero()->add(new Grpc\Timeval(5_000_000));
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/SlowEcho', $d);
    $r = $call->startBatch(fullBatch());
    return 'code=' . $r->status->code;
});

ledger('sleepuntil-honors-span', false, 'slept', 'instant', function () {
    $t0 = microtime(true);
    (new Grpc\Timeval(400_000))->sleepUntil();
    return (microtime(true) - $t0) >= 0.3 ? 'slept' : 'instant';
});

ledger('plugin-invoked-once-per-call', false, '1', '2', function () {
    $n = 0;
    $cc = Grpc\CallCredentials::createFromPlugin(function ($c) use (&$n) { $n++; return []; });
    $call = new Grpc\Call(chan(), '/grpc.testing.TestService/EchoMetadata', dl3());
    $call->setCredentials($cc);
    $call->startBatch([Grpc\OP_SEND_INITIAL_METADATA=>[]]);
    $call->startBatch([Grpc\OP_SEND_MESSAGE=>['message'=>ep('x')]]);
    $call->startBatch([Grpc\OP_SEND_CLOSE_FROM_CLIENT=>true]);
    $call->startBatch([Grpc\OP_RECV_INITIAL_METADATA=>true, Grpc\OP_RECV_MESSAGE=>true, Grpc\OP_RECV_STATUS_ON_CLIENT=>true]);
    return (string)$n;
});

ledger('getpeer-no-io-before-connect', false, 'target-ish', 'fabricated-ip', function () {
    // ext-grpc reports the transport peer with zero I/O; we resolve DNS on the
    // PHP thread and invent an address for a call that never connected.
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    $p = $call->getPeer();
    return (str_starts_with($p, 'ipv4:') || str_starts_with($p, 'ipv6:')) ? 'fabricated-ip' : 'target-ish';
});

ledger('server-side-op-logic-exception', false, 'LogicException(3)', 'InvalidArgumentException(1)', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    try { $call->startBatch([Grpc\OP_RECV_CLOSE_ON_SERVER => true]); return 'accepted'; }
    catch (Throwable $e) { return get_class($e) . '(' . $e->getCode() . ')'; }
});

ledger('string-batch-key-invalid-argument', false, 'InvalidArgumentException(1)', 'Exception(0)', function () {
    $call = new Grpc\Call(chan(), ECHO_M, dl3());
    try { $call->startBatch(['foo' => true]); return 'accepted'; }
    catch (Throwable $e) { return get_class($e) . '(' . $e->getCode() . ')'; }
});

// Weak mode is what real client libraries use; the earlier coercion cases were
// measured under declare(strict_types=1), where ext-grpc also rejects.
ledger('weak-mode-coercion', false, 'int=ok null=ok str=ok', 'int=throw null=throw str=throw', function () {
    $ch = chan();
    $f = function (callable $c) { try { $c(); return 'ok'; } catch (Throwable $e) { return 'throw'; } };
    return sprintf('int=%s null=%s str=%s',
        $f(fn() => $ch->getConnectivityState(1)),
        $f(fn() => @$ch->getConnectivityState(null)),
        $f(fn() => new Grpc\Timeval('1000000')));
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
