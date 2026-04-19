<?php
// Runner-side test: verifies the Server-Sent Events content type survives
// after oxphp_stream_flush(). Does NOT use TestCase/done() because done()
// calls header() after flushing, which raises "headers already sent" on
// streamed responses. The runner asserts on status and content-type only.
declare(strict_types=1);

header('Content-Type: text/event-stream');
echo "data: hello\n\n";
oxphp_stream_flush();
