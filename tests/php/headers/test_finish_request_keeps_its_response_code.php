<?php

declare(strict_types=1);

// Runner-side test: oxphp_finish_request() answers with the status the script
// set with http_response_code() — which PHP records apart from the headers —
// and not with 200.

http_response_code(202);
echo 'accepted';
oxphp_finish_request();
