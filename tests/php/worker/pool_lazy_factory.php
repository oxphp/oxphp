<?php

declare(strict_types=1);

// Included by test_pool_factory_from_file_peer_never_loaded on its own worker
// thread only. Returns a pool factory that calls a function of this file.

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function worker_lazy_pool_helper(int $base = 0): int
{
    return $base + 11;
}

const WORKER_LAZY_POOL_CALL_LINE = __LINE__ + 2;

return static fn (): object => (object) ['v' => worker_lazy_pool_helper()];
