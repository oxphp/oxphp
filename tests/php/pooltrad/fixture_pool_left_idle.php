<?php

declare(strict_types=1);

use OxPHP\Shared\Pool;

// Driven over HTTP by test_pool_destroy_skipped_at_request_end_is_counted;
// answers one JSON object.
//
// Makes a pool and leaves one slot idle in it. The pool's last reference is
// this file's own variable, so the pool goes away as the request ends, on this
// thread, once no PHP code runs here any more. With destroy=1 the pool is given
// a $destroy.
header('Content-Type: application/json');

$pool = new Pool(
    factory: static fn (): \ArrayObject => new \ArrayObject(),
    destroy: ($_GET['destroy'] ?? '') === '1'
        ? static function (\ArrayObject $o): void {}
        : null,
    maxSize: 1,
);
$pool->acquire()->release();

echo json_encode(['worker' => oxphp_worker_id(), 'idle' => $pool->stats()->idle()]);
