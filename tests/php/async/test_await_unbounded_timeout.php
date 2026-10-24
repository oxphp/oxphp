<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_unbounded_timeout', 'async');

// A timeout too large to be a deadline — INF, 1e300, PHP_INT_MAX — waits
// without a limit, as 0.0, a negative timeout and NAN do. Turning such a value
// into a deadline used to abort the whole server (INF or 1e300 in any await,
// PHP_INT_MAX in await_race / await_any inside a fiber), or made the await
// inside a fiber time out at once.
//
// Every await function runs at the top level and inside a task. Outside worker
// mode the top level waits blocking and the task waits in its fiber; in worker
// mode the top level already runs in the request's fiber. Between them that is
// every place a timeout becomes a deadline.
$probe = function (string $which, array $timeouts, float $taskSeconds): array {
    $results = [];
    foreach (['await', 'await_all', 'await_race', 'await_any'] as $fn) {
        foreach ($timeouts as $label => $timeout) {
            $p = oxphp_async(function (float $seconds): int {
                oxphp_sleep($seconds);
                return 42;
            }, $taskSeconds);
            try {
                $results["$fn($label) $which"] = match ($fn) {
                    'await' => oxphp_async_await($p, $timeout),
                    'await_all' => oxphp_async_await_all([$p], $timeout)[$p],
                    'await_race' => oxphp_async_await_race([$p], $timeout)['value'],
                    'await_any' => oxphp_async_await_any([$p], $timeout)['value'],
                };
            } catch (\Throwable $e) {
                $results["$fn($label) $which"] = get_class($e);
            }
        }
    }
    return $results;
};

$unbounded = [
    'INF' => INF,
    '1e300' => 1e300,
    'PHP_INT_MAX' => PHP_INT_MAX,
    'NAN' => NAN,
    '-1.0' => -1.0,
];

// The task settles after 50 ms: an await that returns its value waited for it
// rather than timing out.
$waited = $probe('at the top level', $unbounded, 0.05)
    + oxphp_async_await(oxphp_async($probe, 'inside a task', $unbounded, 0.05));
foreach ($waited as $case => $result) {
    $t->assertSame("$case waits for the result", $result, 42);
}

// The other half: a finite timeout still bounds the wait, on every one of the
// same paths — a task that takes a second is given up on after 0.2 s.
$bounded = $probe('at the top level', ['0.2' => 0.2], 1.0)
    + oxphp_async_await(oxphp_async($probe, 'inside a task', ['0.2' => 0.2], 1.0));
foreach ($bounded as $case => $result) {
    $t->assertSame("$case times out", $result, \OxPHP\Async\TimeoutException::class);
}

$t->done();
