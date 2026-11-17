<?php

declare(strict_types=1);

// A fatal inside serialize() past the request's handler does not leave the next
// serialize() on the stack the table of the call it ended. The engine counts the
// serialize() calls open on the thread and shares one table among nested ones;
// a fatal while one is open leaves the count raised, and the next call — taken
// for one nested inside the dead call — numbers its back-references after the
// dead call's values. Written into a session, such a back-reference names the
// wrong value when the session is read.
//
// Two places the request still runs code after such a fatal: the write of its
// session, after a fatal in a shutdown function; and the save handler's
// close(), after a fatal in the write itself.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/late_fatal_probe.php';

$t = new TestCase('serialize_state_after_a_late_fatal', 'fibers');
$id = bin2hex(random_bytes(6));
$fixture = '/tests/fibers/fixture_serialize_after_late_fatal.php';

$sid = "oxphpcellsshutdown$id";
$written = fiber_inner_request("$fixture?role=shutdown&sid=$sid", 8.0);
// The answer goes out before the shutdown functions run, so the session is
// written after this has it. Read once the write has made the file.
$file = ini_get('session.save_path') . "/sess_$sid";
$deadline = microtime(true) + 5.0;
while (!is_file($file) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$readBody = fiber_inner_request("$fixture?role=read&sid=$sid", 8.0);
$read = json_decode($readBody, true);
$t->meta('session', $read ?? $readBody);
$t->assertContains('the request that failed in a shutdown function answered first', $written, 'answered');
$t->assertSame('its session kept the first key', $read['a'] ?? null, 'stdClass');
$t->assertTrue('and the second key is the same object', $read['same'] ?? null);

OxphpLateFatalProbe::$close = null;
$closed = fiber_inner_request("$fixture?role=session_close&sid=oxphpcellsclose$id", 8.0);
$probe = OxphpLateFatalProbe::$close;
$t->meta('close_probe', $probe);
$t->assertContains('the request that failed writing its session answered first', $closed, 'answered');
$t->assertTrue('its save handler was closed after the write failed', is_array($probe));
$t->assertTrue('a back-reference in close() names the object of its own call', $probe['same'] ?? null);
$t->assertTrue('and its __unserialize() ran', $probe['filled'] ?? null);

$t->done();
