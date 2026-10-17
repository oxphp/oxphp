<?php
// The whole body has to reach the client at oxphp_finish_request(), while the
// script is still running.
header('Content-Type: application/octet-stream');
readfile('/tmp/mid.bin');
oxphp_finish_request();
sleep(3);
