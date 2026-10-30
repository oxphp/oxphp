<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_last_error_is_not_shared_with_a_sibling', 'asyncnocache');

// Tasks on one async thread take turns at its last error. A task that sleeps
// keeps its own and finds it again when it wakes; a task that runs meanwhile
// starts with none. A copy a task took of another's would outlive what it
// points into once that task's request ends. The sleeping task raises once
// before its first sleep and once after waking from it, and a sibling reads
// during each sleep.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$sleeping = oxphp_async(static function (): array {
    @trigger_error('oxphp-error-of-the-sleeping-task', E_USER_WARNING);
    $before = microtime(true);
    oxphp_sleep(1.0);
    $own = error_get_last()['message'] ?? null;

    @trigger_error('oxphp-error-raised-after-waking', E_USER_WARNING);
    $again = microtime(true);
    oxphp_sleep(1.0);

    return [$before, $again, microtime(true), $own, error_get_last()['message'] ?? null];
});
$sibling = static fn (): array => oxphp_async_await(oxphp_async(static function (): array {
    $readAt = microtime(true);
    $seen = error_get_last()['message'] ?? null;
    @trigger_error('oxphp-error-of-the-sibling', E_USER_WARNING);

    return [$readAt, $seen];
}));

[$readAt, $seen] = $sibling();
usleep(1_300_000);
[$readAgainAt, $seenAgain] = $sibling();
[$before, $again, $after, $own, $ownAgain] = oxphp_async_await($sleeping);

$t->assertTrue('the first sibling runs while the task sleeps the first time', $before < $readAt && $readAt < $again);
$t->assertNull('the first sibling finds no error of the sleeping task', $seen);
$t->assertSame('the sleeping task finds its own error when it wakes', $own, 'oxphp-error-of-the-sleeping-task');
$t->assertTrue('the second sibling runs while the task sleeps again', $again < $readAgainAt && $readAgainAt < $after);
$t->assertNull('the second sibling finds no error the task raised after waking', $seenAgain);
$t->assertSame('the task finds the error it raised after waking', $ownAgain, 'oxphp-error-raised-after-waking');

$t->done();
