<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('sse_output_format', 'streaming');

header('Content-Type: text/event-stream');
echo "data: hello\n\n";
oxphp_stream_flush();

$t->assertTrue('oxphp_is_streaming() === true after SSE flush', oxphp_is_streaming() === true);

$t->done();
