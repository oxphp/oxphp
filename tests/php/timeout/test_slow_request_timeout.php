<?php

declare(strict_types=1);

// This script intentionally sleeps longer than the server's REQUEST_TIMEOUT_SECONDS.
// The server will return 408 Request Timeout before this script completes.
// The runner checks for the 408 status code — not the output of this script.
sleep(5);
echo 'should never reach here';
