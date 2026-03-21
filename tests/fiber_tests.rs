//! Integration tests for fiber-based request multiplexing.
//! These tests require a running OxPHP Docker instance with worker mode + async pool.
//! Run with: cargo test --test fiber_tests -- --ignored

#[cfg(test)]
mod fiber_integration {
    /// Two concurrent requests using oxphp_async_await should both complete
    /// in ~200ms (parallel), not ~400ms (serialized).
    #[test]
    #[ignore] // requires docker
    fn async_await_does_not_block_worker() {
        // Placeholder — requires running Docker instance
        // Test validates that fiber-aware await allows concurrent request handling
    }

    /// Two concurrent SSE connections on a single-worker instance should
    /// both stream simultaneously via cooperative oxphp_sleep.
    #[test]
    #[ignore] // requires docker
    fn concurrent_sse_on_single_worker() {
        // Placeholder — requires running Docker instance
        // Test validates that oxphp_sleep yields fiber, allowing multiplexing
    }

    /// Existing non-fiber handlers must work identically (backward compat).
    #[test]
    #[ignore] // requires docker
    fn non_fiber_handler_still_works() {
        // Placeholder — requires running Docker instance
    }
}
