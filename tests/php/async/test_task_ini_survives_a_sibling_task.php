<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// An oxphp_async() task keeps an ini value it set across its own suspension,
// while another task starts on the same async worker.
//
// This profile has one async worker, so both tasks run on one thread: the first
// sets precision and sleeps, and the second starts while it sleeps. The task
// scheduler moves no ini between tasks, so the first task's value is on the
// thread for the second to see — and it is still there, unchanged, when the
// first wakes. Nothing that clears the thread's ini as a request is entered may
// run as a task is entered.

$t = new TestCase('task_ini_survives_a_sibling_task', 'async');

$first = oxphp_async(function (): string {
    ini_set('precision', '5');
    oxphp_sleep(0.1);
    $seen = (string) ini_get('precision');
    // Put back before returning: nothing else does it on a task thread.
    ini_restore('precision');
    return $seen;
});
$second = oxphp_async(function (): int {
    return 1;
});

$t->assertSame('the second task ran', oxphp_async_await($second), 1);
$t->assertSame('the first task woke with the precision it set', oxphp_async_await($first), '5');

$t->done();
