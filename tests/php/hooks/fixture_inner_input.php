<?php

declare(strict_types=1);

// Inner self-request for the two hooks/test_php_input_* tests. It is served only
// while the outer request's fiber is parked in a hooked sleep(), which is the
// window those tests need: reading php://input here leaves this request's body
// stream — and the flag saying its body has already been read in full — standing
// in the thread-wide SAPI state that the parked request comes back to.
//
// Echoes what it read, so the outer test can assert on two things at once: that
// this request ran inside the window at all, and that it got its own body rather
// than the outer one's. Echo-style on purpose — the outer test asserts on this
// body, not on JSON.
//
// Must not suspend: no await, no sleep, no socket read. A suspend here would
// park this fiber too, and the outer request could resume before this one had
// touched php://input at all — which empties the test out silently, since the
// state it is supposed to leave behind would never be written.

$body = file_get_contents('php://input');

if (!is_string($body) || !str_contains($body, 'intruder-body')) {
    http_response_code(500);
    echo 'INNER-FAIL: php://input did not return this request body: '
        . var_export($body, true) . "\n";
    return;
}

echo "INNER-OK $body\n";
