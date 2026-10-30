<?php
// Runs a task on the async pool (ASYNC_WORKERS > 0) and awaits it: the result
// only comes back if a pool thread started its PHP request under the one-shot
// CLI SAPI and executed the closure there.
$p = oxphp_async(static fn (int $x): int => $x + 1, 41);
echo oxphp_async_await($p, 5.0), "\n";
