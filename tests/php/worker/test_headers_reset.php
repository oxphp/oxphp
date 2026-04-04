<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('headers_reset', 'worker');

// At the start of a fresh request, headers_list() should be empty —
// no stale headers from a previous request handled by the same worker.
$headers = headers_list();
$t->assertSame('headers_list() is empty at request start', $headers, []);

$t->done();
