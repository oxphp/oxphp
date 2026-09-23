<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/ini_put_back_probe.php';

// Follows fibers/test_ini_put_back_reports_a_throw. Without the destructor
// having run, that line's 500 would be some other failure's.

$t = new TestCase('ini_put_back_reports_a_throw_probe', 'fibers');

$t->assertTrue('the destructor the put-back ran threw', OxphpIniPutBackProbe::$throwerRan);
$t->assertSame('assert.callback is back to what the worker started with', ini_get('assert.callback'), '');

$t->done();
