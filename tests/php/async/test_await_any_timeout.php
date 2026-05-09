<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_timeout', 'async');

// p1 rejects at 50ms; p2 and p3 sleep 10s each.
// timeout 200ms — p1's error captured as partial; p2+p3 cancelled.
$p1 = oxphp_async(function (): never {
    usleep(50_000);
    throw new \RuntimeException('quick fail');
});
$p2 = oxphp_async(function (): int {
    usleep(10_000_000);
    return 2;
});
$p3 = oxphp_async(function (): int {
    usleep(10_000_000);
    return 3;
});

$caught = null;
try {
    oxphp_async_await_any([$p1, $p2, $p3], 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $caught = $e;
}

$t->assertNotNull('caught TimeoutException', $caught);
if ($caught !== null) {
    $t->assertInstanceOf('is TimeoutException', $caught, \OxPHP\Async\TimeoutException::class);

    $partial = $caught->getPartialErrors();
    $t->assertCount('one partial error captured', $partial, 1);
    $t->assertKeyExists("partial keyed by p1 id", $partial, $p1);
    if (array_key_exists($p1, $partial)) {
        $t->assertContains('partial error message', $partial[$p1]->getMessage(), 'quick fail');
    }

    $pending = $caught->getPendingPromiseIds();
    $t->assertCount('two pending ids', $pending, 2);
    // Order of pending may not match input order; check membership both ways.
    $t->assertTrue("pending contains p2 id", in_array($p2, $pending, true));
    $t->assertTrue("pending contains p3 id", in_array($p3, $pending, true));
}

$t->done();
