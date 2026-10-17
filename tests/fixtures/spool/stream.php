<?php
// Output written before the response turned into a stream has to come first,
// in order, ahead of what the stream sends. A write past the threshold after
// that is streamed, not spooled.
header('Content-Type: application/octet-stream');
readfile('/tmp/mid.bin');
oxphp_stream_flush();
readfile('/tmp/mid.bin');
echo 'TAIL';
// Flushed explicitly: outside Worker mode, output written after the last
// flush of a stream is not sent at all, and that is not what this checks.
flush();
