<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('trace_callback', 'apm');

// oxphp_apm_trace currently does not invoke the callback (no-op handler).
// Verify it completes without error and returns null.
$result = oxphp_apm_trace('test_span', function(): int {
    return 42;
});

$t->assertSame('oxphp_apm_trace returns null (callback not yet wired)', $result, null);

$t->done();
