<?php

declare(strict_types=1);

// Inner self-request for fibers/test_ob_survives_suspend. It runs on the worker
// while the outer request is parked in a hooked sleep with an output buffer of
// its own open.
//
// Both lines below are the point. The buffer stack is thread-wide, so a request
// that starts while another one's buffer is still on it reports a level it never
// opened — and everything it echoes lands in that buffer instead of its own
// response, which is then a response with no body at all.

header('Content-Type: text/plain');
echo 'INNER-OB-LEVEL:', ob_get_level(), "\n";
echo "INNER-BODY\n";
