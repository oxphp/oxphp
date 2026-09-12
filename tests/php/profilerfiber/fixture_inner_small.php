<?php

declare(strict_types=1);

// Inner self-request for profilerfiber/test_cap_stays_with_its_request. Profiled
// (the outer test sends the trigger header on the socket) and deliberately tiny:
// the outer request has already burned past PROFILER_MAX_SPANS by the time this
// one is admitted, so what this request records says whether the span counter
// the cap is measured against belongs to a request or to the thread.
//
// Echo-style on purpose — the outer test asserts on this body, not on JSON.
//
// Must not suspend, for the same reason as fixture_inner_profiled.php.

if (!function_exists('pf_small_fn')) {
    function pf_small_fn(int $n): int
    {
        return $n - 1;
    }
}

$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += pf_small_fn($i);
}

if (!OxPHP\Profile\is_active()) {
    http_response_code(500);
    echo "INNER-SMALL-FAIL: the trigger did not activate profiling here\n";
    return;
}

echo "INNER-SMALL-OK sum=$sum\n";
