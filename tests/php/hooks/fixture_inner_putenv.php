<?php

declare(strict_types=1);

// Inner self-request for hooks/test_putenv_rolled_back ?mode=park. It can only
// be served while the outer request's fiber is parked in a hooked sleep: it is
// the request that ends while another one on the same worker is still in
// flight.
//
// It reports the outer request's variable as it finds it, sets one of its own,
// and ends. The outer test asserts on this body.
//
// Must not suspend — no await, no sleep, no socket read. This request exists to
// run to completion inside the outer one's window.

if (!putenv('OX_PUTENV_INNER=inner-value')) {
    http_response_code(500);
    echo "INNER-FAIL: putenv of the inner variable failed\n";
    return;
}

echo 'INNER-OK ' . json_encode(['outer' => getenv('OX_PUTENV_OUTER')]) . "\n";
