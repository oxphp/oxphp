<?php

declare(strict_types=1);

// For test_curl_multi_getcontent_of_a_finished_transfer_makes_no_copy: `?kb=`
// kilobytes of "x", sent after `?delay=` milliseconds. The usleep() is hooked, so
// the worker runs the request reading it meanwhile.

$delay = (int) ($_GET['delay'] ?? 0);
if ($delay > 0) {
    usleep($delay * 1000);
}
echo str_repeat('x', (int) ($_GET['kb'] ?? 1) * 1024);
