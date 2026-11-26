<?php

declare(strict_types=1);

// Runner-side test: a stream started with oxphp_stream_start() that fails
// before its first write has sent nothing, so the failure is answered with a
// 500, as a buffered response's is.

if (!function_exists('oxphp_stream_start')) {
    echo "FAIL: oxphp_stream_start() does not exist\n";

    return;
}

oxphp_stream_start();
header('Content-Type: text/csv');

throw new RuntimeException('failed before the first write');
