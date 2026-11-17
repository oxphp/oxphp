<?php

declare(strict_types=1);

// A fatal inside serialize() raised in code the worker runs outside any request
// does not leave the next serialize() there the table of the call it ended. The
// worker releases what an async task closed over once the task has ended, and a
// task that outlives the request that started it is released between requests,
// on the worker's own stack. A fatal inside serialize() in a destructor that
// release runs leaves the engine's count of open calls raised on that stack,
// and every later serialize() the worker runs there numbers its back-references
// after the dead call's values.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/late_fatal_probe.php';

$t = new TestCase('serialize_state_on_the_worker_after_a_fatal', 'fibers');
$fixture = '/tests/fibers/fixture_serialize_after_late_fatal.php?role=drain';
$base = sys_get_temp_dir() . '/oxphp_cells_drain_' . bin2hex(random_bytes(6));

/** Waits for a file the worker writes between requests; this request parks meanwhile. */
$waitFor = static function (string $file): bool {
    for ($i = 0; $i < 400 && !is_file($file); $i++) {
        oxphp_usleep(20_000);
    }

    return is_file($file);
};

// A fatal flags every object alive on the thread as already destructed, so the
// object that records is made by a request that starts once the fatal is over.
$failed = fiber_inner_request("$fixture&what=fatal&file=" . rawurlencode("$base-a"), 8.0);
$t->assertContains('the request whose task fails answered', $failed, 'dispatched');
$t->assertTrue('the release of what that task held ran its destructor', $waitFor("$base-a.fatal"));

$probed = fiber_inner_request("$fixture&what=probe&file=" . rawurlencode("$base-b"), 8.0);
$t->assertContains('the request whose task records answered', $probed, 'dispatched');
$probe = $waitFor("$base-b") ? json_decode((string) file_get_contents("$base-b"), true) : null;
$t->meta('probe', $probe);
$t->assertTrue('the release of what the second task held recorded what it saw', is_array($probe));
$t->assertTrue('a back-reference there names the object of its own call', $probe['same'] ?? null);
$t->assertTrue('and its __unserialize() ran', $probe['filled'] ?? null);

foreach (glob("$base-*") ?: [] as $leftover) {
    unlink($leftover);
}

$t->done();
