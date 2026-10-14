<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// Preloaded functions and classes, anonymous ones included, exist on every
// thread: a task written in the preloaded file runs, and so does one written
// here that uses what the preloaded file declares.
$t = new TestCase('test_task_runs_preloaded_code', 'asyncdecl');

$tasks = asyncdecl_pre_tasks() + [
    'call from here' => static fn (): int => asyncdecl_pre_helper(),
    'named from here' => static fn (): int => (new AsyncDeclPreNamed())->x,
];
$results = [];
foreach ($tasks as $name => $task) {
    $results[$name] = asyncdecl_settle(oxphp_async($task));
}

$t->assertSame('each task returns', $results, [
    'call' => [true, 11],
    'anon' => [true, 13],
    'named' => [true, 12],
    'call from here' => [true, 11],
    'named from here' => [true, 12],
]);

$t->done();
