<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('trace_callback_disabled', 'apm');

// This test runs on the `default` profile, where OTEL_APM_ENABLED is unset. The
// APM plugin still registers its PHP functions there, and oxphp_apm_trace must
// keep running the callback — a tracing helper that swallows business logic when
// tracing happens to be off is the same defect as one that swallows it always.

$t->assertTrue(
    'oxphp_apm_trace is registered even with APM disabled',
    function_exists('oxphp_apm_trace')
);

// Premise, read before anything depends on it: APM really is off in this
// profile. Otherwise the assertions below would be exercising the enabled
// branch and reporting a pass for a path they never took.
$t->assertSame('APM is disabled in this profile', oxphp_apm_start('probe'), 0);

$called = false;
$spanArg = null;

$result = oxphp_apm_trace('disabled.child', function (int $span) use (&$called, &$spanArg): string {
    $called = true;
    $spanArg = $span;
    return 'ran-anyway';
}, ['component' => 'trace-callback-disabled-test']);

$t->assertTrue('the callback is invoked with APM disabled', $called);
$t->assertSame('the callback return value is forwarded', $result, 'ran-anyway');
$t->assertSame('the callback receives span id 0 when APM is disabled', $spanArg, 0);

// The exception path stays a plain rethrow: no span exists to mark.
$thrown = null;
try {
    oxphp_apm_trace('disabled.throws', function (int $span): void {
        throw new RuntimeException('disabled boom');
    });
} catch (\Throwable $e) {
    $thrown = $e;
}

$t->assertNotNull('the callback exception propagates with APM disabled', $thrown);
$t->assertSame(
    'the propagated exception is the one the callback threw',
    $thrown?->getMessage(),
    'disabled boom'
);

// Refusing a non-callable is part of the same contract, and the disabled branch
// reaches it through its own call site — the enabled test proves nothing about
// this one.
$typeError = null;
try {
    /** @phpstan-ignore-next-line — deliberately not a callable */
    oxphp_apm_trace('disabled.bad', 'oxphp_no_such_function_zzz');
} catch (\TypeError $e) {
    $typeError = $e;
}

$t->assertNotNull('a non-callable second argument throws TypeError with APM disabled', $typeError);
$t->assertContains(
    'the TypeError names oxphp_apm_trace and the callback argument position',
    $typeError?->getMessage() ?? '',
    'oxphp_apm_trace(): Argument #2 ($callback)'
);

$t->done();
