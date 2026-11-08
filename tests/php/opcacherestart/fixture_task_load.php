<?php

declare(strict_types=1);

// One request of the load in test_load_then_reset_under_task_load: tasks that
// park, so that a pool thread always has some in flight. Each stays busy briefly
// before it parks, as a task does before its first I/O: the thread awake first
// would otherwise take the whole batch and leave the other one idle until the
// next. They park for different times, so that no moment finds them all ended.

header('Content-Type: application/json');
$task = static function (float $park): void {
    $until = microtime(true) + 0.002;
    while (microtime(true) < $until) {
    }
    oxphp_sleep($park);
};
$ids = [];
for ($i = 0; $i < 12; $i++) {
    $ids[] = oxphp_async($task, mt_rand(150, 450) / 1000);
}
oxphp_async_await_all($ids, 10.0);
echo '{}';
