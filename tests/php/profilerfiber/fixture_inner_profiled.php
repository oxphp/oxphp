<?php

declare(strict_types=1);

// Inner self-request for profilerfiber/test_spans_stay_with_their_request. It is
// profiled in its own right (the outer test sends the trigger header on the
// socket), and it can only be served while the outer request's fiber is parked,
// so the two profiled requests are alive on the same PHP thread at once.
//
// Calls one function nobody else calls, which is how the reader test tells whose
// spans a run holds.
//
// Echo-style on purpose — the outer test asserts on this body, not on JSON.
//
// Must not suspend: a suspend point here would park this request too and the
// outer one would resume while this one is still open, which is a different
// interleaving from the one the test is written for.

if (!function_exists('pf_inner_fn')) {
    function pf_inner_fn(int $n): int
    {
        return $n + 1;
    }
}

$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += pf_inner_fn($i);
}

if (!OxPHP\Profile\is_active()) {
    http_response_code(500);
    echo "INNER-PROFILED-FAIL: the trigger did not activate profiling here\n";
    return;
}

echo "INNER-PROFILED-OK sum=$sum\n";
