<?php

declare(strict_types=1);

// Inner self-request for profilerfiber/test_apm_neighbour_leaves_profile_alone.
// Carries no trigger, so it is the ordinary traffic a profiled request shares a
// worker with: ApmOnly in this build, never ProfileAll. It must neither take the
// parked request's spans nor leave a run of its own behind.
//
// Echo-style on purpose — the outer test asserts on this body, not on JSON.
//
// Must not suspend, for the same reason as fixture_inner_profiled.php.

if (!function_exists('pf_plain_fn')) {
    function pf_plain_fn(int $n): int
    {
        return $n * 3;
    }
}

$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += pf_plain_fn($i);
}

if (OxPHP\Profile\is_active()) {
    http_response_code(500);
    echo "INNER-PLAIN-FAIL: untriggered request is recording a full profile\n";
    return;
}

echo "INNER-PLAIN-OK sum=$sum\n";
