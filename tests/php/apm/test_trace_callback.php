<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('trace_callback', 'apm');

$result = oxphp_apm_trace('test_span', function(): int {
    return 42;
});

$t->assertSame('oxphp_apm_trace returns closure result', $result, 42);

$t->done();
