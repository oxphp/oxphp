<?php
// Runner-side test: a streamed response with a compressible content type
// must bypass brotli compression even when the client sends
// Accept-Encoding: br. Buffering-for-compression would destroy
// time-to-first-byte and show up here as Content-Encoding: br instead of
// Transfer-Encoding: chunked. Does NOT use TestCase/done() because done()
// calls header() after flushing, which raises "headers already sent".
// The runner asserts: 200, transfer-encoding:chunked, content-encoding missing.
declare(strict_types=1);

header('Content-Type: text/html; charset=utf-8');

// Body must exceed MIN_COMPRESS_SIZE (256 bytes) so a buffered response
// WOULD be compressed — proving the streaming bypass, not the size gate.
echo str_repeat('<p>chunk one</p>', 20);
oxphp_stream_flush();

echo str_repeat('<p>chunk two</p>', 20);
oxphp_stream_flush();
