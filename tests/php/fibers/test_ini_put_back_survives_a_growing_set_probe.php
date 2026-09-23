<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/ini_put_back_probe.php';

// Follows fibers/test_ini_put_back_survives_a_growing_set.

// Read first: the task has to be running as this request is taken, or the worker
// was idle in between and rolled back what the assertions below look for.
$taskRunning = !is_file(OxphpIniPutBackProbe::GROWER_TASK_DONE);

$t = new TestCase('ini_put_back_survives_a_growing_set_probe', 'fibers');

$t->assertTrue('the task the last request left was still running as this one was taken', $taskRunning);

$t->assertTrue('the destructor the put-back ran set its directives', OxphpIniPutBackProbe::$growerRan);

$damaged = [];
foreach (OxphpIniPutBackProbe::FILL_LENGTHS as $i => $length) {
    if ((OxphpIniPutBackProbe::$fill[$i] ?? null) !== str_repeat(chr(0x61 + $i), $length)) {
        $damaged[] = $length;
    }
}
$t->assertSame('nothing wrote into what the destructor allocated afterwards', $damaged, []);

$standing = [];
foreach (OxphpIniPutBackProbe::GROWN as $name => $value) {
    if (ini_get($name) === $value) {
        $standing[] = $name;
    }
}
$t->assertSame('none of what it set is standing in this request', $standing, []);
$t->assertSame('assert.callback is back to what the worker started with', ini_get('assert.callback'), '');

$t->done();
