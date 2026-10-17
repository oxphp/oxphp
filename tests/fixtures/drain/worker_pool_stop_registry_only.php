<?php
// Scenario J3 of tests/graceful_drain.sh: as worker_pool_stop.php, but the
// named pool is reached only inside the request handler. Once the requests are
// done the registry holds its last reference, so the server's stop drops it on
// the main thread, while its factory and destroy closures live in the heap of
// the worker that created it.
oxphp_worker(static function (): void {
    $pool = \OxPHP\Shared\Registry::pool('drain-stop', static fn (): \OxPHP\Shared\Pool => new \OxPHP\Shared\Pool(
        factory: static fn (): \stdClass => new \stdClass(),
        destroy: static function (\stdClass $r): void {
            error_log('drain-stop: destroy ran');
        },
        maxSize: 4,
    ));
    $h = $pool->acquire();
    usleep(300_000);
    $h->release();
    echo 'pool-parked';
});
