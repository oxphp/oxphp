<?php

declare(strict_types=1);

// Which worker and which OS thread served this request.
header('Content-Type: application/json');
echo json_encode([
    'worker' => oxphp_worker_id(),
    'thread' => readlink('/proc/thread-self'),
]);
