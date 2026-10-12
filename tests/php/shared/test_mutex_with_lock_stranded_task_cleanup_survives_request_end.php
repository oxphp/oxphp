<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('mutex_with_lock_stranded_task_cleanup_survives_request_end', 'shared');

// A task an await gave up on was cancelled then, and its `finally` may still
// be running when the request ends. The end of the request cancels the tasks
// nobody awaited; it must not cancel this one a second time, or the `finally`
// takes a second "Async task cancelled" — here, as a withLock() that gives up
// on the lock it is waiting for.
//
// The fixture is a separate request so that its end can be observed from here.
// Outside worker mode the end of the request waits for the stranded task, so
// how its cleanup ended is final once the response is in.
//
// Needs ASYNC_WORKERS >= 2: the lock holder and the stranded task have to run
// on different threads. Needs PHP_WORKERS >= 2: the fixture request runs while
// this one waits for it.
$id = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . "/oxphp_stranded_cleanup_{$id}";

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fixture socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 12);
fwrite($sock, "GET /tests/shared/fixture_stranded_cleanup_lock.php?id={$id} HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
$resp = (string) stream_get_contents($sock);
fclose($sock);

$t->assertContains(
    "the request ended while the stranded task's cleanup was waiting for the lock",
    $resp,
    'cleanup-still-waiting'
);

clearstatcache();
$result = file_exists("{$base}.result")
    ? (string) file_get_contents("{$base}.result")
    : '(no result)';
$t->assertSame("the stranded task's cleanup waited for the lock and ran", $result, 'acquired');

foreach (glob("{$base}.*") ?: [] as $file) {
    unlink($file);
}
$t->done();
