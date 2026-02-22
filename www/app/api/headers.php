<?php

json_response(200, [
    'request_headers'  => request_headers(),
    'response_headers' => [
        'note' => 'Check response headers in your HTTP client for: Server, X-Request-ID, Content-Encoding, etc.',
    ],
]);
