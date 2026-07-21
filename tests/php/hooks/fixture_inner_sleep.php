<?php

declare(strict_types=1);

// Served in its own request fiber; hooked sleep suspends it.
sleep(1);
echo 'inner-done';
