<?php

declare(strict_types=1);

require_once __DIR__ . '/check.php';

oxphp_worker(function (): void {
    entry_only_check(__FILE__);
});
