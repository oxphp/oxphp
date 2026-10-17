<?php
// Worker-mode fixture for Scenario J of tests/graceful_drain.sh: a Shared\Pool
// whose slots are idle on the workers when the server stops. Every request
// holds a slot for a moment, so two at once are served by both workers and
// each worker mints a slot of its own.
$pool = \OxPHP\Shared\Registry::pool('drain-stop', static fn (): \OxPHP\Shared\Pool => new \OxPHP\Shared\Pool(
    factory: static fn (): \stdClass => new \stdClass(),
    destroy: static function (\stdClass $r): void {
        error_log('drain-stop: destroy ran');
    },
    maxSize: 4,
));

oxphp_worker(static function () use ($pool): void {
    $h = $pool->acquire();
    usleep(300_000);
    $h->release();
    echo 'pool-parked';
});
