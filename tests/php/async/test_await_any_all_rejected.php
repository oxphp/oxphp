<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_all_rejected', 'async');

// Three promises, all throw. await_any must collect all three errors and throw
// AggregateAsyncException carrying every one.
$promises = [];
foreach (['A', 'B', 'C'] as $tag) {
    $promises[] = oxphp_async(function (string $tag): never {
        throw new \RuntimeException("rejection $tag");
    }, $tag);
}

$caught = null;
try {
    oxphp_async_await_any($promises, 5.0);
} catch (\OxPHP\Async\AggregateAsyncException $e) {
    $caught = $e;
}

$t->assertNotNull('caught AggregateAsyncException', $caught);
if ($caught !== null) {
    $t->assertInstanceOf('is AggregateAsyncException', $caught, \OxPHP\Async\AggregateAsyncException::class);
    $errors = $caught->getErrors();
    $t->assertCount('three errors collected', $errors, 3);

    if (count($errors) === 3) {
        $t->assertContains('first error tagged A', $errors[0]->getMessage(), 'rejection A');
        $t->assertContains('second error tagged B', $errors[1]->getMessage(), 'rejection B');
        $t->assertContains('third error tagged C', $errors[2]->getMessage(), 'rejection C');
    }

    $map = $caught->getErrorMap();
    $t->assertCount('errorMap has three entries', $map, 3);
    foreach ($promises as $pid) {
        $t->assertKeyExists("errorMap contains promise $pid", $map, $pid);
    }

    $ids = $caught->getPromiseIds();
    $t->assertCount('promiseIds has three entries', $ids, 3);
    $t->assertSame('promiseIds matches input order', $ids, $promises);
}

$t->done();
