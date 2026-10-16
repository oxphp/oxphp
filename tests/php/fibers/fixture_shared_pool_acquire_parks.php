<?php

declare(strict_types=1);

// Inner self-request for fibers/test_shared_pool_acquire_parks: the waiter.
//
// Asks a saturated pool for a slot while the test request, on the same worker
// thread, holds it. Answers with what the wait ended in: the name the holder
// wrote on the resource it handed over, or the class of what was thrown.

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

header('Content-Type: application/json');

$name = (string) ($_GET['pool'] ?? '');
$mode = (string) ($_GET['mode'] ?? '');

$p = Registry::pool(
    $name,
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), maxSize: 1)
);

try {
    $got = match ($mode) {
        'timeout' => (static function (Pool $p): ?string {
            $h = $p->acquireTimeout(3000);
            $got = $h->get()->name ?? null;
            $h->release();

            return $got;
        })($p),
        'with' => $p->withTimeout(static fn (\stdClass $r): ?string => $r->name ?? null, 3000),
        'forever' => (static function (Pool $p): ?string {
            $h = $p->acquire();
            $got = $h->get()->name ?? null;
            $h->release();

            return $got;
        })($p),
        default => throw new \LogicException("unknown mode '$mode'"),
    };
    echo json_encode(['result' => 'acquired', 'name' => $got]);
} catch (\Throwable $e) {
    echo json_encode(['result' => get_class($e), 'message' => $e->getMessage()]);
}
