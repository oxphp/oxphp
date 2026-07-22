<?php
// Fixture for test_orphan_drain_nonblocking: dispatch a fire-and-forget async
// task that BLOCKS the async worker in a native syscall, then return without
// awaiting. time_nanosleep() is NOT covered by RUNTIME_HOOKS (only sleep()/
// usleep() are), so it stays native and is not cooperatively cancellable — the
// promise's receiver cannot settle until the 2s sleep finishes. This models a
// request that finishes while still owning a still-running async task, so its
// promise cleanup must be deferred rather than block the worker scheduler.
oxphp_async(function (): void {
    time_nanosleep(2, 0); // 2s native blocking sleep on the async worker
});

// Yield briefly so the async worker dequeues and STARTS the task (enters the
// native sleep) before this request finalizes. Without this the request's
// finalize would flip the promise's cancel flag before the task is dequeued,
// so the async pool skips it pre-execution (async_pool.rs) — the promise
// settles instantly and nothing blocks, hiding the scheduler stall we test.
oxphp_usleep(100000); // 0.1s

echo 'orphan-dispatched';
