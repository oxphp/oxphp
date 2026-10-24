<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_rolls_back_beside_other_requests_handles', 'hooksdb');

// From PHP 8.4.27 and 8.5.12 on, PDO leaves a pooled connection's transaction
// open when a handle on it is destroyed while other handles on it remain: those
// are taken to be the same script's, which carries on with the transaction
// through them. In worker mode the others are as often an earlier request's,
// kept in a static property, which will never touch that transaction. Each case
// below drops a request's own handle beside such a handle and checks the
// transaction went with it, as it did before those releases.
$fetch = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_user.php?{$query} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return $body;
};

// 1, 2. The request holding the connection drops the handle it opened the
// transaction through, while an earlier request's handle is kept on the same
// connection — by unset(), and by an exception unwinding the frame it lives in.
// The request also keeps a handle of its own on another connection, and runs on
// the fiber the earlier request ran on: neither makes the kept handle its own.
// An exception the rollback is sent and read under still reaches the catch
// around the call.
foreach (['unset', 'throw'] as $how) {
    $key = 'dtor-own-' . $how . '-' . bin2hex(random_bytes(4));
    $aside = 'dtor-own-aside-' . bin2hex(random_bytes(4));
    $built = $fetch('step=build&key=' . $key);
    $dropped = $fetch('step=drop-own&how=' . $how . '&key=' . $key . '&aside=' . $aside);
    preg_match('/built: fiber:(\d+)/', $built, $b);
    preg_match('/ fiber:(\d+) /', $dropped, $d);
    preg_match('/ conn:(\d+)\/(\d+) /', $dropped, $c);

    $t->assertContains("{$how}: an earlier request's handle was kept on the connection", $dropped, 'had:true');
    $t->assertSame("{$how}: on the fiber this request ran on", $d[1] ?? 'none', $b[1] ?? 'unbuilt');
    $t->assertContains(
        "{$how}: the handle was dropped the way this case means",
        $dropped,
        $how === 'throw' ? 'threw:true' : 'threw:false'
    );
    $t->assertContains(
        "{$how}: the catch around the call ran only for the exception",
        $dropped,
        $how === 'throw' ? 'caught:true' : 'caught:false'
    );
    $t->assertSame("{$how}: and the transaction was checked on the connection it was opened on", $c[2] ?? 'none', $c[1] ?? 'unseen');
    $t->assertContains(
        "{$how}: dropping its own handle rolled the transaction back",
        $dropped,
        'in_txn:false rows:0'
    );
}

// 3. A request ends with a transaction open through one of two handles it keeps,
// and this request — which holds nothing — drops that one. The other handle is
// the ended request's too, but nothing is left to carry on through it.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$openKey = 'dtor-open-' . bin2hex(random_bytes(4));
$opened = $fetch('step=open&key=' . $openKey);
preg_match('/ conn:(\d+)/', $opened, $o);
$t->assertContains('an earlier request ended with its transaction open', $opened, 'persistent-dtor-user-opened:');

OxDtorUserBox::$pdo = null;

$after = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_PERSISTENT => $openKey,
]);
$inTxn = $after->inTransaction();
$rows = (string) $after->query('SELECT COUNT(*) FROM ox_dtor_open')->fetchColumn();
$t->assertSame(
    'checked on the connection it was opened on',
    (string) $after->query('SELECT CONNECTION_ID()')->fetchColumn(),
    $o[1] ?? 'unseen'
);
if ($inTxn) {
    $after->rollBack();
}
OxDtorUserBox::$other = null;
$t->assertFalse('dropping one of the ended request\'s handles rolled its transaction back', $inTxn);
$t->assertSame('and the row it inserted is gone', $rows, '0');

$t->done();
