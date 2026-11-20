<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Arms the worker entry's code after the loop to raise a fatal error inside
// the session extension's request shutdown, then asks the worker to leave: a
// worker's thread runs that shutdown only as it exits. The next suite line is
// served by the worker that replaced this one, if the server is still up.

$test = new TestCase('rshutdown_fatal_arm', 'recycle');

$tmp = sys_get_temp_dir();
foreach (['closed', 'nosession'] as $leftover) {
    if (is_file("$tmp/oxphp-rshutdown-fatal-worker.$leftover")) {
        unlink("$tmp/oxphp-rshutdown-fatal-worker.$leftover");
    }
}
touch("$tmp/oxphp-entry-rshutdown-fatal.armed");

$worker = OxPHP\Server\Worker::current();
$worker->scheduleExit();
$test->assertTrue('the exit is scheduled', $worker->isExitScheduled());

$test->done();
