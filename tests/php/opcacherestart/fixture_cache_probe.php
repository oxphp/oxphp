<?php

declare(strict_types=1);

header('Content-Type: application/json');
echo json_encode(['cached' => opcache_is_script_cached(__FILE__)]);
