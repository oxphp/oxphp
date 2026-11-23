<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../fibers/multisort_probe.php';

$t = new TestCase('persistent_ctor_gate_refuses_where_switching_is_blocked', 'hooksdb');

// A persistent PDO constructor that finds another fiber on its worker inside a
// constructor it has to wait for, at a point where fiber switching is blocked —
// a comparison of array_multisort() here — can neither wait nor go ahead. The
// fiber it would wait for is parked on this same thread and cannot run until
// this code returns, so a wait would only spend its budget; and going ahead
// unserialised is the race in the pool the wait exists to prevent. It is
// refused with a PDOException instead.
//
// The holder is a constructor to a server that accepts the connection and never
// answers, on this worker's own 127.0.0.1:3306. The constructor under test names
// the healthy database through a `uri:` file, which waits for a constructor to
// any data source: without the refusal it would connect.
//
// Two constructors bound the refusal. One inside a Fiber this request started,
// with the holder still inside, cannot wait there either but is not refused:
// switching is allowed, only the context is not the server's, and it goes ahead
// as it did before the gate existed. And one in a comparison again, once the
// holder has left, finds the way clear and is built.
$db = getenv('DB_NAME') ?: 'appdb';
$dbHost = getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql';
$server = @stream_socket_server('tcp://127.0.0.1:3306', $errno, $errstr);
if ($server === false) {
    $t->assertTrue("the black-hole listener bound: {$errstr} ({$errno})", false);
    $t->done();
    return;
}

$sharedState['black_hole_dsn'] = "mysql:host=127.0.0.1;port=3306;dbname={$db}";
unset($sharedState['black_hole_entered'], $sharedState['black_hole_left']);

$uriFile = '/tmp/oxphp-ctor-dsn-' . bin2hex(random_bytes(4));
file_put_contents($uriFile, "mysql:host={$dbHost};port=3306;dbname={$db}");

$request = function (string $path, int $timeout): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, $timeout);
    fwrite($sock, "GET /tests/hooksdb/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
};

$holder = oxphp_async($request, 'fixture_persistent_ctor_black_hole.php', 20);

// Until the holder is inside its constructor, and then long enough for it to be
// parked on the greeting it will never get rather than still connecting.
for ($i = 0; $i < 100 && !isset($sharedState['black_hole_entered']); $i++) {
    oxphp_usleep(20_000);
}
oxphp_usleep(200_000);

// [outcome, seconds spent], and whether the holder was inside as it started.
$construct = static function (string $tag) use ($uriFile, &$sharedState): array {
    $holderInside = isset($sharedState['black_hole_entered']) && !isset($sharedState['black_hole_left']);

    // PHP 8.5 deprecates reading a DSN from a URI; the notice is not what this
    // looks for.
    set_error_handler(static fn (): bool => true, E_DEPRECATED);
    $start = microtime(true);
    try {
        new PDO('uri:file://' . $uriFile, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_PERSISTENT => $tag . '-' . bin2hex(random_bytes(4)),
        ]);
        $outcome = 'built';
    } catch (\Throwable $e) {
        $outcome = get_class($e) . ': ' . $e->getMessage();
    } finally {
        $elapsed = microtime(true) - $start;
        restore_error_handler();
    }

    return [$outcome, $elapsed, $holderInside];
};

$blocked = null;
multisort_probe_sort(static function () use ($construct, &$blocked): void {
    $blocked = $construct('ctor-blocked');
});
[$outcome, $elapsed, $holderInside] = $blocked;

$t->assertTrue('premise: the black-hole constructor was inside PDO when the one under test ran', $holderInside);
$t->assertSame(
    'the constructor is refused rather than built unserialised',
    $outcome,
    'PDOException: persistent connection not built: another fiber on this worker is inside a persistent constructor this one has to wait for, and the code running here cannot be suspended, so it can neither wait nor go ahead'
);
$t->assertLessThan(sprintf('and the refusal spent no wait budget (%.3fs)', (float) $elapsed), (float) $elapsed, 0.2);

// PHP 8.6.0 starts a Fiber at error_reporting 0 when php.ini does not set it,
// and at 0 a deprecation that skipped the handler is not printed either: the
// constructor would pass there whichever mode it ran in.
$fiber = new Fiber(static function () use ($construct): array {
    error_reporting(E_ALL);

    return $construct('ctor-userland');
});
[$outcome, $elapsed, $holderInside] = $fiber->start() ?? $fiber->getReturn();
$t->assertTrue('premise: the holder was still inside for the one in a Fiber', $holderInside);
$t->assertSame('one in a Fiber this request started goes ahead instead', $outcome, 'built');

// Closing the listener resets the connection the holder is parked on, so it
// leaves its constructor with an error instead of after a day.
fclose($server);
$held = oxphp_async_await($holder)['body'];
$t->assertContains('the black-hole constructor ended in a connection error', $held, 'black-hole-failed:');

$clear = null;
multisort_probe_sort(static function () use ($construct, &$clear): void {
    $clear = $construct('ctor-clear');
});
@unlink($uriFile);
$t->assertFalse('premise: the holder had left for the last one', $clear[2]);
$t->assertSame('one in a comparison that finds the way clear is built', $clear[0], 'built');

$after = oxphp_async($request, 'fixture_db_sleep.php', 10);
$t->assertContains('the worker served the next request as usual', oxphp_async_await($after)['body'], 'db-done');

$t->done();
