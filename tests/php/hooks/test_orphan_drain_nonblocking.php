<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('orphan_drain_nonblocking', 'hooks');

// Regression: a request that finishes while still owning a fire-and-forget
// async task must not freeze the worker scheduler while that task's promise is
// cleaned up. PHP_WORKERS=1 guarantees the bystander request below is served by
// THIS worker thread while this fiber is suspended. The bystander dispatches a
// 2s native-blocking async task (not cooperatively cancellable) and returns
// without awaiting; its own finalize used to block_on that promise (up to 5s)
// on the worker thread, freezing the scheduler — so this fiber's 0.3s sleep
// could not resume until the bystander's task finished (~2s). With deferred
// drain the bystander finalizes immediately and this sleep resumes on schedule.
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('bystander socket connected', $sock !== false);
stream_set_timeout($sock, 6);
fwrite($sock, "GET /tests/hooks/fixture_orphan_blocking.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Suspend this fiber briefly. While suspended, the scheduler accepts and
// finalizes the bystander request on this same worker thread.
$t0 = microtime(true);
oxphp_usleep(300000); // 0.3s cooperative sleep
$elapsed = microtime(true) - $t0;

$resp = (string) stream_get_contents($sock);
fclose($sock);

$t->assertContains('bystander request was served on this worker', $resp, 'orphan-dispatched');
$t->assertTrue(
    'this fiber\'s 0.3s sleep resumed on schedule, not stalled behind the bystander\'s promise drain (~2s)',
    $elapsed < 1.5
);
$t->assertGreaterThan('the 0.3s sleep actually elapsed', $elapsed, 0.25);

$t->done();
