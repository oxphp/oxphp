<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_class_lookup_slot_of_a_script_from_the_file_cache', 'asyncshmfull');

// While OPcache's shared memory is full, a script it loads from its file cache
// stays in the memory of the request, and the class names in it get
// class-lookup slots the PHP thread numbers on its own. Each PHP worker below
// loads one fixture that way and numbers its one name from the same start, and
// the tasks they start both run on the single async thread. That thread must not
// take what it looked up for one worker's name as the class of the other's.

// Strings of this script, which OPcache caches in its shared memory before the
// fill below, so a fixture's parameter type and its `new` resolve to this one
// interned string, and `new` looks the class up in the slot the type numbers.
$spellings = ['arrayObject', 'splStack'];

$fill = '/tmp/asyncshmfull-fill';
if (!is_dir($fill)) {
    mkdir($fill);
}
for ($i = 0; $i < 1000 && !opcache_get_status(false)['cache_full']; $i++) {
    file_put_contents("$fill/$i.php", "<?php return $i;\n");
    include "$fill/$i.php";
}
$t->assertTrue("OPcache's table is full", opcache_get_status(false)['cache_full']);

$fixture = __DIR__ . '/fixture_lookup_array_object.php';
$first = include $fixture;
$t->assertTrue('the fixture is not in shared memory', !opcache_is_script_cached($fixture));
$t->assertTrue('OPcache stored it in its file cache', glob("/tmp/*$fixture.bin") !== []);
$second = include $fixture;
$t->assertSame('the tasks of both includes build their class', "$first $second", 'ArrayObject ArrayObject');

$other = @file_get_contents(
    'http://127.0.0.1/tests/asyncshmfull/fixture_lookup_spl_stack_twice.php',
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);
$t->assertSame("the other worker's tasks build their own class", $other, 'SplStack SplStack');

$t->done();
