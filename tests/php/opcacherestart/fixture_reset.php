<?php

declare(strict_types=1);

require_once __DIR__ . '/lib.php';

header('Content-Type: application/json');
$reset = opcache_reset();
echo json_encode(['reset' => $reset] + ox_opcache_state());
