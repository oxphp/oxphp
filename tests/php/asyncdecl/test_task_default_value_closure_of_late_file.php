<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';
require_once __DIR__ . '/bulk.php';

// PHP 8.5 accepts a static closure and a first-class callable in a
// parameter's default value, and each has a per-thread slot of its own. A task
// whose parameters default to them, from a file this worker thread included
// after a file of thousands of functions: OPcache numbered those slots past
// the slots the task's thread had. Both defaults are evaluated and called,
// and the server stays up.
$t = new TestCase('test_task_default_value_closure_of_late_file', 'asyncdecl');

if (PHP_VERSION_ID < 80500) {
    $t->meta('php', 'PHP 8.4 has no closures or first-class callables in constant expressions');
    $t->assertTrue('nothing to check before PHP 8.5', true);
    $t->done();
}

$bulk = asyncdecl_bulk('defaults');
$fixture = __DIR__ . '/fixture_late_default_value.php';
$task = require $fixture;
[$ok, $result] = asyncdecl_settle(oxphp_async($task));

$t->assertTrue('the generated file is cached', opcache_is_script_cached($bulk));
$t->assertTrue('the task\'s file is cached', opcache_is_script_cached($fixture));
$t->assertTrue('the task returns', $ok);
$t->assertSame('what its defaults computed', $result, [42, 3]);

$t->done();
