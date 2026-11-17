<?php

declare(strict_types=1);

// Inner request for
// fibers/test_a_channel_wait_without_a_timeout_throws_where_a_request_cannot_park.
// Each ?case= calls Channel::recv() or send(), which have no timeout, on the
// channel the driver leaves in $sharedState['forever_channel'], from a point
// where this request cannot park:
//
// - filter-recv, filter-send and the filter-* cases that need no wait: in a
//   FILTER_CALLBACK of filter_input_array() given a definitions array (fiber
//   switching blocked);
// - multisort-recv: in a comparison of array_multisort() (fiber switching
//   blocked);
// - multisort-fiber-recv: the same, with the sort run inside a userland Fiber
//   this request starts, so on a context nested in the request's own;
// - multisort-gc-recv: in the destructor of a cycle that a comparison collects
//   with gc_collect_cycles(), which the engine runs on its own destructor fiber,
//   nested in the request's;
// - superglobal-recv: in the destructor of an object this request leaves in
//   $_GET, which runs as the worker gives the request's superglobals back (the
//   server's parking blocked).
//
// The outcome goes to $sharedState under forever_<case>.

require_once __DIR__ . '/multisort_probe.php';

$case = (string) ($_GET['case'] ?? '');
$channel = $sharedState['forever_channel'];

$probe = static function () use ($case, $channel): array {
    $isSend = str_starts_with($case, 'filter-send');
    $start = microtime(true);
    try {
        $result = $isSend ? $channel->send('more') : $channel->recv();
        $outcome = $result->isOk() && !$isSend
            ? 'value: ' . $result->value()
            : 'status: ' . $result->status()->name;
    } catch (\Throwable $e) {
        $outcome = get_class($e) . ': ' . $e->getMessage();
    }

    return ['outcome' => $outcome, 'elapsed' => microtime(true) - $start];
};

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file.
$destructs = static fn (\Closure $onDestruct): object => new class ($onDestruct) {
    public ?object $self = null;

    public function __construct(private \Closure $onDestruct)
    {
    }

    public function __destruct()
    {
        ($this->onDestruct)();
    }
};

if ($case === 'superglobal-recv') {
    $_GET['held'] = $destructs(static function () use ($probe, &$sharedState): void {
        $sharedState['forever_superglobal-recv'] = $probe();
    });
} elseif ($case === 'multisort-recv') {
    multisort_probe_sort(static function () use ($probe, &$sharedState): void {
        $sharedState['forever_multisort-recv'] = $probe();
    });
} elseif ($case === 'multisort-fiber-recv') {
    (new Fiber(static function () use ($probe, &$sharedState): void {
        multisort_probe_sort(static function () use ($probe, &$sharedState): void {
            $sharedState['forever_multisort-fiber-recv'] = $probe();
        });
    }))->start();
} elseif ($case === 'multisort-gc-recv') {
    multisort_probe_sort(static function () use ($probe, $destructs, &$sharedState): void {
        $requestFiber = Fiber::getCurrent();
        $collecting = true;
        $cycle = $destructs(static function () use ($probe, $requestFiber, &$collecting, &$sharedState): void {
            $sharedState['forever_multisort-gc-recv'] = $probe() + [
                'nested' => Fiber::getCurrent() !== $requestFiber,
                'collecting' => $collecting,
            ];
        });
        $cycle->self = $cycle;
        unset($cycle);
        gc_collect_cycles();
        $collecting = false;
    });
} else {
    filter_input_array(INPUT_GET, ['k' => ['filter' => FILTER_CALLBACK, 'options' => static function (string $v) use ($probe, $case, &$sharedState): string {
        $sharedState["forever_$case"] = $probe();

        return $v;
    }]]);
}
