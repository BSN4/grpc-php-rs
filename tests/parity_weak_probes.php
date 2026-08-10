<?php
// NO declare(strict_types=1) here — deliberately. Real client libraries run
// in weak typing mode; the parity ledger's weak-mode-coercion case includes
// this file so its probes execute with genuine weak-mode coercion semantics
// (the ledger file itself is strict, which would change what is measured).

function weak_mode_probes(Grpc\Channel $ch): string {
    $f = function (callable $c) {
        try { $c(); return 'ok'; }
        catch (Throwable $e) {
            $short = substr(strrchr('\\' . get_class($e), '\\'), 1);
            return 'throw:' . $short;
        }
    };
    return sprintf('int=%s null=%s str=%s',
        $f(fn() => $ch->getConnectivityState(1)),
        $f(fn() => @$ch->getConnectivityState(null)),
        $f(fn() => new Grpc\Timeval('1000000')));
}
