<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Registry;
use OxPHP\Shared\SharedException;

// The session this request carries was decoded while the request started up
// (session.auto_start), and the object in it called Registry::pool() from
// __wakeup. This is the first request on its thread, so nothing earlier there
// has said how long the thread's requests last: the startup has to know it
// already, or that call binds a name to a pool that lives in this request's
// memory and is gone when the request ends.
$t = new TestCase('test_registry_refuses_pool_during_startup', 'poolstartup');

$t->assertTrue('the session object woke up', PoolStartupWake::$woken);
$t->assertSame('it woke up with no PHP frame below it, before any script', PoolStartupWake::$frames, 1);
$t->assertSame('Registry::pool() is refused there', PoolStartupWake::$threw, SharedException::class);
$t->assertContains('the refusal names the method', PoolStartupWake::$message, 'Registry::pool() cannot be called from a request served without worker mode');
$t->assertFalse('the name stays free', in_array('poolstartup', Registry::keys(), true));

$t->done();
