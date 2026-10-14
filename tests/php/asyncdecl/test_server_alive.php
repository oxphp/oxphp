<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// Runs last: every task above was answered by a process still serving.
$t = new TestCase('test_server_alive', 'asyncdecl');

[$ok, $result] = asyncdecl_settle(oxphp_async(static fn (): int => 42));
$t->assertSame('a task still runs', [$ok, $result], [true, 42]);

$t->done();
