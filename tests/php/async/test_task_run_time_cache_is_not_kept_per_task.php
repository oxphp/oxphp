<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A task runs a closure of its own, which the engine gives a run-time cache of
// its own. The task's thread runs every task inside the one request it opened
// at start, so a second cache per task would stay until the thread exits.
//
// This profile has one async worker, so every task runs on one thread, and the
// closure's 40 property writes and 40 reads give it a cache of a few kilobytes.
// They are made on json_decode()'s result: on a `new \stdClass()` OPcache's
// optimizer leaves the closure a cache of a few dozen bytes.
$t = new TestCase('test_task_run_time_cache_is_not_kept_per_task', 'async');

$task = static function (): array {
    $o = json_decode('{}');
    $o->a = 0;
    $o->b = 1;
    $o->c = 2;
    $o->d = 3;
    $o->e = 4;
    $o->f = 5;
    $o->g = 6;
    $o->h = 7;
    $o->i = 8;
    $o->j = 9;
    $o->k = 10;
    $o->l = 11;
    $o->m = 12;
    $o->n = 13;
    $o->o = 14;
    $o->p = 15;
    $o->q = 16;
    $o->r = 17;
    $o->s = 18;
    $o->t = 19;
    $o->u = 20;
    $o->v = 21;
    $o->w = 22;
    $o->x = 23;
    $o->y = 24;
    $o->z = 25;
    $o->ba = 26;
    $o->bb = 27;
    $o->bc = 28;
    $o->bd = 29;
    $o->be = 30;
    $o->bf = 31;
    $o->bg = 32;
    $o->bh = 33;
    $o->bi = 34;
    $o->bj = 35;
    $o->bk = 36;
    $o->bl = 37;
    $o->bm = 38;
    $o->bn = 39;
    $sum = $o->a + $o->b + $o->c + $o->d + $o->e + $o->f + $o->g + $o->h + $o->i + $o->j +
        $o->k + $o->l + $o->m + $o->n + $o->o + $o->p + $o->q + $o->r + $o->s + $o->t + $o->u +
        $o->v + $o->w + $o->x + $o->y + $o->z + $o->ba + $o->bb + $o->bc + $o->bd + $o->be +
        $o->bf + $o->bg + $o->bh + $o->bi + $o->bj + $o->bk + $o->bl + $o->bm + $o->bn;
    return [memory_get_usage(), $sum];
};

$usage = static fn (): int => oxphp_async_await(oxphp_async($task))[0];

for ($i = 0; $i < 50; $i++) {
    $usage();
}
$before = $usage();
for ($i = 51; $i < 1050; $i++) {
    $usage();
}
$after = $usage();
$perTask = intdiv($after - $before, 1000);
$t->assertTrue("memory of the task's thread grows by less than 128 bytes a task (grew by $perTask)", $perTask < 128);

$t->done();
