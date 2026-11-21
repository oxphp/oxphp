<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_class_lookup_slot_after_the_pool_thread_compiled_names', 'asyncnocache');

// Without OPcache, every PHP thread numbers the class-lookup slots of the class
// names it compiles on its own, from the same start. A task that includes a
// file has the async thread compile it and number its names, and the next
// tasks run code another PHP thread compiled, whose names carry numbers from
// the same start. The async thread must not take the class it looked up for
// its own name as the class of theirs. This runs first in its profile, on the
// single async thread before any other task included a file there.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$t->assertSame(
    'a task has the async thread compile names of its own and fill their slots',
    oxphp_async_await(oxphp_async(static fn (): array => include __DIR__ . '/fixture_lookup_names_the_pool_thread_compiles.php')),
    array_fill(0, 8, 'SplStack'),
);

$get = static fn (string $fixture): string|false => @file_get_contents(
    "http://127.0.0.1/tests/asyncnocache/$fixture.php",
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);

$t->assertSame('the next task builds its own class', $get('fixture_lookup_array_object'), 'ArrayObject');
$t->assertSame('and so does the one after it', $get('fixture_lookup_spl_stack'), 'SplStack');

$t->done();
