<?php

declare(strict_types=1);

// For test_curl_multi_getcontent_mid_transfer_keeps_its_string: an answer in two
// parts, 100 bytes of "a" sent at once and 50 bytes of "b" 300 ms later. The
// event-stream type makes it a stream, so the first part goes out on the flush
// rather than with the rest; the usleep() between them is hooked, so the worker
// runs the request reading it meanwhile.

header('Content-Type: text/event-stream');
echo str_repeat('a', 100);
oxphp_stream_flush();
usleep(300_000);
echo str_repeat('b', 50);
