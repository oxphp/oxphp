<?php

declare(strict_types=1);

// Inner request for fibers/test_shared_pool_acquire_ended_by_a_recycle: waits
// for the slot of a pool the test holds, and is still waiting when the test has
// its worker exit, which ends it where it waits. Its shutdown function reports
// to a file, because the worker's memory goes with the worker, and output from a
// cancelled request raises the cancellation again at the first write.

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

$name = (string) ($_GET['pool'] ?? '');

$p = Registry::pool(
    $name,
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), maxSize: 1)
);

register_shutdown_function(static function () use ($p): void {
    file_put_contents('/tmp/oxphp-fibers-pool-acquire-recycled.txt', json_encode([
        'message' => error_get_last()['message'] ?? '',
        'waiting' => $p->stats()->waiting(),
    ]) . "\n", FILE_APPEND);
});

// Bounded, so that a build whose wait holds the thread gives the test its
// worker back.
try {
    $p->acquireTimeout(5000)->release();
    echo 'acquired';
} catch (\Throwable $e) {
    echo get_class($e);
}
