<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_gc_keeps_holder_transaction', 'hooksdb');

// A handle this request leaves in a garbage cycle is destroyed by whichever fiber
// next runs the cycle collector — here, the request holding the connection, in
// the middle of its transaction. Being destroyed on the holder's fiber does not
// make it the holder's handle: it must leave that transaction alone just as it
// would have if this request had dropped it directly.
$sharedState['dtor_gc_key'] = 'dtor-gc-collect-' . bin2hex(random_bytes(4));
unset(
    $sharedState['dtor_gc_parked'],
    $sharedState['dtor_gc_dropped'],
    $sharedState['dtor_gc_released'],
    $sharedState['dtor_gc_sentinel'],
    $sharedState['dtor_gc_collector']
);

$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_gc_hold.php?mode=collect HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['dtor_gc_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request opened its transaction before this one woke',
    $sharedState['dtor_gc_parked'] ?? false
);

$error = '';
$stillHeld = false;
$keep = null;
try {
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['dtor_gc_key'],
    ];
    $keep = new PDO($dsn, $user, $pass, $opts);

    $cycle = new stdClass();
    $cycle->self = $cycle;
    $cycle->pdo = new PDO($dsn, $user, $pass, $opts);
    $cycle->sentinel = new class ($sharedState) {
        private array $state;

        public function __construct(array &$state)
        {
            $this->state = &$state;
        }

        public function __destruct()
        {
            $this->state['dtor_gc_sentinel'] = $this->state['dtor_gc_collector'] ?? 'none';
        }
    };
    // Garbage now, and left for the collector rather than freed here.
    unset($cycle);

    $stillHeld = !($sharedState['dtor_gc_released'] ?? false);
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}
$sharedState['dtor_gc_dropped'] = true;

$t->assertSame('this request built its handles: ' . $error, $error, '');
$t->assertTrue('and left one as garbage while the holder was inside its transaction', $stillHeld);

$body = oxphp_async_await($task)['body'];

// The premise: the holder's own collection is what destroyed this request's
// handle, inside the holder's transaction.
$t->assertContains('the holder\'s collector destroyed the garbage handle', $body, 'collected_by:holder');

$t->assertContains(
    'and the holder found its transaction still open and committed it',
    $body,
    'in_txn:true rows:1'
);

preg_match('/ id:(\d+)$/', $body, $m);
$mine = $keep !== null ? (string) $keep->query('SELECT CONNECTION_ID()')->fetchColumn() : '';
$t->assertSame(
    'and the garbage handle was one on the holder\'s connection',
    $mine,
    $m[1] ?? 'the holder printed no connection id'
);
unset($keep);

$t->done();
