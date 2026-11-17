<?php

declare(strict_types=1);

// Inner request for fibers/test_waits_on_the_thread_where_a_request_cannot_park.
// Each ?case= reaches a wait from a point where this request cannot park, and
// reports how the wait went and whether a Fiber could switch there:
//
// - filter-channel: Channel::recvTimeout() in a FILTER_CALLBACK of
//   filter_input_array() given a definitions array (fiber switching blocked);
// - multisort-channel: the same in a comparison of array_multisort() (fiber
//   switching blocked);
// - superglobal-channel: the same in the destructor of an object this request
//   leaves in $_GET, which runs as the worker gives the request's superglobals
//   back (the server's parking blocked, fiber switching not);
// - filter-await-any, filter-await-race: oxphp_async_await_any() or
//   oxphp_async_await_race() over two tasks that are not done yet, in a
//   FILTER_CALLBACK.
//
// The first three report into $sharedState under wait_<case>, since the third
// runs after anything it could echo has been sent; the last two echo.

require_once __DIR__ . '/multisort_probe.php';

$case = (string) ($_GET['case'] ?? '');

$probe = static function (): array {
    $fiber = new Fiber(static fn () => Fiber::suspend('switched'));
    try {
        $switch = $fiber->start();
    } catch (\FiberError $e) {
        $switch = 'FiberError: ' . $e->getMessage();
    }
    if ($fiber->isSuspended()) {
        $fiber->resume();
    }

    $channel = new OxPHP\Shared\Channel(1);
    $start = microtime(true);
    try {
        $wait = $channel->recvTimeout(100)->status()->name;
    } catch (\Throwable $e) {
        $wait = get_class($e) . ': ' . $e->getMessage();
    }

    $end = microtime(true);

    return ['switch' => $switch, 'wait' => $wait, 'elapsed' => $end - $start, 'window' => [$start, $end]];
};

if ($case === 'filter-channel') {
    filter_input_array(INPUT_GET, ['k' => ['filter' => FILTER_CALLBACK, 'options' => static function (string $v) use ($probe, &$sharedState): string {
        $sharedState['wait_filter-channel'] = $probe();

        return $v;
    }]]);

    return;
}

if ($case === 'multisort-channel') {
    multisort_probe_sort(static function () use ($probe, &$sharedState): void {
        $sharedState['wait_multisort-channel'] = $probe();
    });

    return;
}

if ($case === 'superglobal-channel') {
    // Anonymous: a named class declared here would be declared again by the next
    // request on this worker to include this file.
    $_GET['held'] = new class (static function () use ($probe, &$sharedState): void {
        $sharedState['wait_superglobal-channel'] = $probe();
    }) {
        public function __construct(private \Closure $onDestruct)
        {
        }

        public function __destruct()
        {
            ($this->onDestruct)();
        }
    };

    return;
}

if ($case === 'filter-await-any' || $case === 'filter-await-race') {
    $await = $case === 'filter-await-any' ? 'oxphp_async_await_any' : 'oxphp_async_await_race';
    $result = null;
    filter_input_array(INPUT_GET, ['k' => ['filter' => FILTER_CALLBACK, 'options' => static function (string $v) use (&$result, $await): string {
        $first = oxphp_async(static function (): int {
            oxphp_usleep(100_000);

            return 1;
        });
        $second = oxphp_async(static function (): int {
            oxphp_usleep(100_000);

            return 2;
        });
        $start = microtime(true);
        try {
            $winner = $await([$first, $second], 5.0);
            $wait = $winner['value'];
        } catch (\Throwable $e) {
            $winner = null;
            $wait = get_class($e) . ': ' . $e->getMessage();
        }
        $result = ['wait' => $wait, 'elapsed' => microtime(true) - $start];
        if ($winner !== null) {
            oxphp_async_await($winner['id'] === $first ? $second : $first, 5.0);
        }

        return $v;
    }]]);
    echo json_encode($result);
}
