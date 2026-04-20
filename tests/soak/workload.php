<?php
/**
 * Synthetic pool workload for pool_soak.sh.
 *
 * Keeps 10 pools alive across every request (handles bound to
 * apcu-style per-worker globals via the WORKER_FILE trick — set
 * WORKER_FILE=/tests/soak/workload.php for this to actually run as
 * outer-scope). Each GET / drives `acquire + work + release` on a
 * randomly chosen pool; the "work" is a 50 us busy loop so the slot
 * genuinely transitions idle → in_use → idle on a real time axis.
 *
 * Knobs (read from env so the compose file can set them):
 *   SOAK_POOL_COUNT       number of Shared\Pool instances (default 10)
 *   SOAK_POOL_MAX_SIZE    maxSize per pool                 (default 8)
 *   SOAK_IDLE_TIMEOUT     pool idleTimeout in seconds      (default 2)
 *
 * The short idle timeout is deliberate — we want the eviction
 * scheduler to fire continuously so the soak exercises the
 * drain-on-eviction path, not just hot acquire/release.
 */

$count   = (int) (getenv('SOAK_POOL_COUNT')    ?: 10);
$maxSize = (int) (getenv('SOAK_POOL_MAX_SIZE') ?: 8);
$idleSec = (float) (getenv('SOAK_IDLE_TIMEOUT') ?: 2.0);

// Outer scope: runs once per worker. Build N pools with a trivial
// factory and a destroy callback that increments a Counter so the
// verify step can prove destroy was actually called.
$pools     = [];
$destroyed = new OxPHP\Shared\Counter();

for ($i = 0; $i < $count; $i++) {
    $pools[] = new OxPHP\Shared\Pool(
        factory: function () use ($i) {
            $r = new stdClass();
            $r->pool  = $i;
            $r->born  = microtime(true);
            return $r;
        },
        destroy: function (object $_r) use ($destroyed) {
            $destroyed->inc();
        },
        maxSize: $maxSize,
        idleTimeout: $idleSec,
    );
}

oxphp_worker(function () use ($pools, $destroyed, $count) {
    $which = random_int(0, $count - 1);
    $pool  = $pools[$which];

    // Short acquire timeout so a saturated pool returns 429-ish
    // promptly rather than building a queue. Soak sizing should
    // keep saturation below 100% on average.
    try {
        $r = $pool->with(function ($slot) {
            // 50 us of CPU work so idle/in_use counts oscillate
            // on a realistic time axis rather than flickering at
            // FFI-round-trip speed.
            $deadline = microtime(true) + 0.00005;
            while (microtime(true) < $deadline) {
                // busy-spin
            }
            return $slot->born;
        }, timeout: 0.25);
    } catch (OxPHP\Shared\TimeoutException $e) {
        http_response_code(503);
        header('Retry-After: 1');
        echo "saturated pool={$which}\n";
        return;
    }

    header('Content-Type: text/plain');
    printf(
        "ok pool=%d size=%d in_use=%d idle=%d waiting=%d destroyed=%d\n",
        $which,
        $pool->size(),
        $pool->inUse(),
        $pool->idle(),
        $pool->waiting(),
        $destroyed->get(),
    );
});
