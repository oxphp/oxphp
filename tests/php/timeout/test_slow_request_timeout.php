<?php

declare(strict_types=1);

// Drive cancellation through PHP's native execution-time budget. SIGALRM
// fires at the 1-second mark, oxphp's interrupt handler converts it to
// CancelReason::Timeout, and the unified bailout produces a 500 response
// before this script can return.
set_time_limit(1);
sleep(5);
echo 'should never reach here';
