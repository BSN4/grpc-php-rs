<?php
declare(strict_types=1);

$tests = 0;
$passed = 0;

function check(string $name, bool $result): void {
    global $tests, $passed;
    $tests++;
    if ($result) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}\n"; }
}

echo "=== API Surface Tests ===\n";

check('extension loaded', extension_loaded('grpc'));

// Classes exist
check('Grpc\\Channel exists', class_exists('Grpc\\Channel'));
check('Grpc\\ChannelCredentials exists', class_exists('Grpc\\ChannelCredentials'));
check('Grpc\\CallCredentials exists', class_exists('Grpc\\CallCredentials'));
check('Grpc\\Timeval exists', class_exists('Grpc\\Timeval'));
check('Grpc\\Call exists', class_exists('Grpc\\Call'));

// Constants exist
check('STATUS_OK = 0', defined('Grpc\\STATUS_OK') && Grpc\STATUS_OK === 0);
check('STATUS_UNAUTHENTICATED = 16', defined('Grpc\\STATUS_UNAUTHENTICATED') && Grpc\STATUS_UNAUTHENTICATED === 16);
check('CHANNEL_IDLE = 0', defined('Grpc\\CHANNEL_IDLE') && Grpc\CHANNEL_IDLE === 0);
check('CHANNEL_READY = 2', defined('Grpc\\CHANNEL_READY') && Grpc\CHANNEL_READY === 2);
check('CHANNEL_FATAL_FAILURE = 4', defined('Grpc\\CHANNEL_FATAL_FAILURE') && Grpc\CHANNEL_FATAL_FAILURE === 4);
check('OP_SEND_INITIAL_METADATA = 0', defined('Grpc\\OP_SEND_INITIAL_METADATA') && Grpc\OP_SEND_INITIAL_METADATA === 0);
check('OP_SEND_MESSAGE = 1', defined('Grpc\\OP_SEND_MESSAGE') && Grpc\OP_SEND_MESSAGE === 1);
check('OP_RECV_STATUS_ON_CLIENT = 6', defined('Grpc\\OP_RECV_STATUS_ON_CLIENT') && Grpc\OP_RECV_STATUS_ON_CLIENT === 6);
check('CALL_OK = 0', defined('Grpc\\CALL_OK') && Grpc\CALL_OK === 0);
check('WRITE_NO_COMPRESS = 2', defined('Grpc\\WRITE_NO_COMPRESS') && Grpc\WRITE_NO_COMPRESS === 2);
check('VERSION defined', defined('Grpc\\VERSION') && is_string(Grpc\VERSION));

// ChannelCredentials methods
check('createSsl() callable', method_exists('Grpc\\ChannelCredentials', 'createSsl'));
check('createInsecure() callable', method_exists('Grpc\\ChannelCredentials', 'createInsecure'));
check('createComposite() callable', method_exists('Grpc\\ChannelCredentials', 'createComposite'));
check('createDefault() callable', method_exists('Grpc\\ChannelCredentials', 'createDefault'));
check('setDefaultRootsPem() callable', method_exists('Grpc\\ChannelCredentials', 'setDefaultRootsPem'));
check('isDefaultRootsPemSet() callable', method_exists('Grpc\\ChannelCredentials', 'isDefaultRootsPemSet'));
check('invalidateDefaultRootsPem() callable', method_exists('Grpc\\ChannelCredentials', 'invalidateDefaultRootsPem'));

// createInsecure returns null
$insecure = Grpc\ChannelCredentials::createInsecure();
check('createInsecure() returns null', $insecure === null);

// CallCredentials
check('createFromPlugin() callable', method_exists('Grpc\\CallCredentials', 'createFromPlugin'));

// Timeval
$now = Grpc\Timeval::now();
check('Timeval::now()', $now instanceof Grpc\Timeval);
check('Timeval::infFuture()', Grpc\Timeval::infFuture() instanceof Grpc\Timeval);
check('Timeval::infPast()', Grpc\Timeval::infPast() instanceof Grpc\Timeval);
check('Timeval::zero()', Grpc\Timeval::zero() instanceof Grpc\Timeval);

// Timeval arithmetic
$a = new Grpc\Timeval(1000000);
$b = new Grpc\Timeval(500000);
$sum = $a->add($b);
check('Timeval::add()', $sum instanceof Grpc\Timeval);
$diff = $a->subtract($b);
check('Timeval::subtract()', $diff instanceof Grpc\Timeval);
check('Timeval::compare() a > b', Grpc\Timeval::compare($a, $b) === 1);
check('Timeval::compare() b < a', Grpc\Timeval::compare($b, $a) === -1);
check('Timeval::compare() equal', Grpc\Timeval::compare($a, $a) === 0);
$threshold = new Grpc\Timeval(600000);
check('Timeval::similar() true', Grpc\Timeval::similar($a, $b, $threshold));
$small_threshold = new Grpc\Timeval(100);
check('Timeval::similar() false', !Grpc\Timeval::similar($a, $b, $small_threshold));

// Insecure channel (no network needed)
$ch = new Grpc\Channel('localhost:50051', ['credentials' => $insecure]);
check('Channel created', $ch instanceof Grpc\Channel);
check('getTarget()', str_contains($ch->getTarget(), 'localhost'));
check('getConnectivityState()', is_int($ch->getConnectivityState()));
$ch->close();
check('close() no error', true);

// CallCredentials plugin callback
$callCreds = Grpc\CallCredentials::createFromPlugin(function (string $serviceUrl) {
    return ['authorization' => 'Bearer test-token'];
});
check('createFromPlugin()', $callCreds instanceof Grpc\CallCredentials);

// Composite credentials
$sslCreds = Grpc\ChannelCredentials::createSsl();
check('createSsl() works', $sslCreds instanceof Grpc\ChannelCredentials);

$composite = Grpc\ChannelCredentials::createComposite($sslCreds, $callCreds);
check('createComposite()', $composite instanceof Grpc\ChannelCredentials);

// Default roots PEM management
check('isDefaultRootsPemSet() initially false', !Grpc\ChannelCredentials::isDefaultRootsPemSet());
Grpc\ChannelCredentials::setDefaultRootsPem('--- test pem ---');
check('isDefaultRootsPemSet() after set', Grpc\ChannelCredentials::isDefaultRootsPemSet());
Grpc\ChannelCredentials::invalidateDefaultRootsPem();
check('isDefaultRootsPemSet() after invalidate', !Grpc\ChannelCredentials::isDefaultRootsPemSet());

// Call object creation (with insecure channel, no network)
$ch2 = new Grpc\Channel('localhost:50051', ['credentials' => $insecure]);
$deadline = Grpc\Timeval::infFuture();
$call = new Grpc\Call($ch2, '/test.Service/Method', $deadline);
check('Call created', $call instanceof Grpc\Call);
check('Call::getPeer()', is_string($call->getPeer()));
$call->cancel();
check('Call::cancel() no error', true);
check('Call::setCredentials()', $call->setCredentials($callCreds) === 0);
$ch2->close();

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
