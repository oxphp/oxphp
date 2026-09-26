<?php

declare(strict_types=1);

// Target of a runner-side status check in suites/overflow.txt. The body is
// plain text rather than a TestCase result, so it carries no assertions of its
// own and only the status and timing decide the test.
echo 'ok';
