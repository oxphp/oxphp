<?php
// Runner-side test: oxphp_stream_flush() forces the response into streaming
// mode. Does NOT use TestCase/done() because done() calls header() after
// flushing, which raises "headers already sent". The runner asserts on
// HTTP status and the streaming transfer encoding.
declare(strict_types=1);

echo 'STREAMED';
oxphp_stream_flush();
