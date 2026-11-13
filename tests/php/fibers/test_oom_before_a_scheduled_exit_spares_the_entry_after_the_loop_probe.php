<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/after_loop_probe.php';

// Follows fibers/test_oom_before_a_scheduled_exit_spares_the_entry_after_the_loop,
// on the worker that replaced the one it ran on.

$t = new TestCase('oom_before_a_scheduled_exit_spares_the_entry_after_the_loop_probe', 'fibers');

$base = sys_get_temp_dir() . '/oxphp-oom-before-exit';

// What the rest stands on: the destructor ran.
$t->assertTrue('the destructor of what the request left in $_GET ran', is_file("$base.ran"));

$after = OxphpAfterLoop::wait();
$t->assertNotNull('the worker left its loop and ran the code after it', $after);
$t->assertSame('and a callback ran there', $after, '[2,4]');

OxphpAfterLoop::clear();
if (is_file("$base.ran")) {
    unlink("$base.ran");
}

$t->done();
