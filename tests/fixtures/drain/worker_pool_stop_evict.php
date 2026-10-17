<?php
// Scenario J2 of tests/graceful_drain.sh: as worker_pool_stop.php, but each
// worker makes a pool of its own and evicts its idle slots once its loop has
// ended. A pool from Registry::pool() would not do here: its $destroy is the
// closure of the one worker that created it, freed when that worker's script
// ends, possibly before the other worker gets to its evict().
$pool = new \OxPHP\Shared\Pool(
    factory: static fn (): \stdClass => new \stdClass(),
    destroy: static function (\stdClass $r): void {
        error_log('drain-stop: destroy ran');
    },
    maxSize: 4,
);

oxphp_worker(static function () use ($pool): void {
    $h = $pool->acquire();
    usleep(300_000);
    $h->release();
    echo 'pool-parked';
});

// Code after oxphp_worker() runs when the loop ends, the server's stop among
// the reasons: here a worker can still hand its own idle slots to $destroy.
error_log('drain-stop: evicted ' . $pool->evict());
