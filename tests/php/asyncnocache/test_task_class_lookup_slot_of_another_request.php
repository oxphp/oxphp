<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_class_lookup_slot_of_another_request', 'asyncnocache');

// Without OPcache, a PHP thread numbers the class-lookup slots of the class
// names it compiles on its own, from the same start in every request. Each
// fixture below is compiled by a request of its own on the other PHP worker
// and holds one such name, so both names get the same slot number, and the
// tasks they start both run on the single async thread. That thread must not
// take what it looked up for one request's name as the class of the other's.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$get = static fn (string $fixture): string|false => @file_get_contents(
    "http://127.0.0.1/tests/asyncnocache/$fixture.php",
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);

$t->assertSame('the first task builds its class', $get('fixture_lookup_array_object'), 'ArrayObject');
$t->assertSame('the second task builds its own class', $get('fixture_lookup_spl_stack'), 'SplStack');

$t->done();
