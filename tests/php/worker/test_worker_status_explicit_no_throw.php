<?php

// Worker mode must honor an explicit http_response_code() on a normal response
// (no throw, no streaming). The status a script sets via http_response_code()
// lives only in SG(sapi_headers) and must be collected into the response.
// Expected wire status: 201, not 200.

http_response_code(201);

echo 'created';
