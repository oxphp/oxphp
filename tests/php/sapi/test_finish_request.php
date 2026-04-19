<?php
// Runner-side test: oxphp_finish_request() ends the response so output after
// it never reaches the client. Does NOT use TestCase/done() because done()
// writes headers + JSON, which both contradict the early response we want to
// verify. The runner asserts on status and content type only.
declare(strict_types=1);

header('Content-Type: text/plain');
echo 'BEFORE_FINISH';
oxphp_finish_request();
echo 'AFTER_FINISH';
