<?php
// Streaming boundary. The response commits a 5xx status and starts streaming
// (headers are on the wire), THEN a fatal is thrown. The status ships, but the
// request has already completed once the headers went out, so the late fatal is
// logged only and does NOT reach the root span — the test asserts its absence.
http_response_code(500);
header('Content-Type: text/plain');
echo 'partial';
oxphp_stream_flush(); // sends the 500 headers + first chunk; streaming begins

throw new RuntimeException('stream fatal after headers');
