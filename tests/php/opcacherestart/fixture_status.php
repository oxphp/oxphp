<?php

declare(strict_types=1);

require_once __DIR__ . '/lib.php';

header('Content-Type: application/json');
echo json_encode(ox_opcache_state());
