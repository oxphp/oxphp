<?php

// An explicit http_response_code() set before an uncaught throw must survive onto
// the wire. The error callback stamps a fatal 500 mid-request, but PHP substitutes
// 500 only when the code is still 200 (main/main.c), so the explicit 503 wins.
// Expected wire status: 503, not 500.

http_response_code(503);

throw new RuntimeException('maintenance');
