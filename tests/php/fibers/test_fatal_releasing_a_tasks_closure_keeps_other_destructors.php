<?php

declare(strict_types=1);

// A fatal error raised where no request is running — in a destructor of what a
// task captured, which the worker lets go of between requests once the task a
// finished request left running has stopped — leaves the objects a parked
// request and the worker hold their destructors, as a fatal in a request does.
//
// This request holds an object of its own and one in a static, then parks on
// its inner request, which dispatches such a task, and then on the task's
// stop. The engine flags every object alive on the thread as already
// destructed on its way into that fatal's bailout.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/destruct_probe.php';

$t = new TestCase('fatal_releasing_a_tasks_closure_keeps_other_destructors', 'fibers');

OxphpDestructProbe::reset(['closure_parked', 'closure_kept']);

$parked = new OxphpDestructProbe('closure_parked');
OxphpDestructProbe::$kept = new OxphpDestructProbe('closure_kept');

$run = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . '/oxphp-release-left-' . $run;

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
if ($sock !== false) {
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_fatal_releasing_what_a_request_left.php?step=started&run=$run HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->assertContains('the inner request left a running task holding the object', $resp, 'DISPATCHED');
}

// The closure goes once the cancelled task has stopped; usleep() parks this
// request so the worker can get there. Its destructor writes the marker before
// it fails, and the recovery from that fatal is over before this request runs
// again: nothing parks in between.
for ($i = 0; $i < 100 && !is_file("$base.ran"); $i++) {
    usleep(50_000);
}
$t->assertTrue('the destructor of what the task captured ran', is_file("$base.ran"));

unset($parked);
OxphpDestructProbe::$kept = null;

$t->assertTrue("the destructor of this request's object ran", OxphpDestructProbe::destructed('closure_parked'));
$t->assertTrue("the destructor of the worker's object ran", OxphpDestructProbe::destructed('closure_kept'));

foreach (glob("$base.*") ?: [] as $file) {
    unlink($file);
}

$t->done();
