<?php
// Streaming late-error capture. The response commits a 5xx status and starts
// streaming (headers are on the wire), THEN a fatal is thrown. The status can no
// longer change, but the exception event must still reach the root span — its
// final errors are delivered when the stream closes and RequestComplete is
// deferred until then.
http_response_code(500);
header('Content-Type: text/plain');
echo 'partial';
oxphp_stream_flush(); // sends the 500 headers + first chunk; streaming begins

throw new RuntimeException('stream fatal after headers');
