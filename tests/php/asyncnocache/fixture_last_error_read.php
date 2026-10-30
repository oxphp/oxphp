<?php

declare(strict_types=1);

// Fetched by test_task_last_error_does_not_outlive_its_request once the request
// that raised the warning has ended, on the PHP worker that ran it. Strings the
// size of that request's message and file name take the memory it freed first,
// so a reference still left to them reads these instead.
$spacers = [];
$fileLength = strlen(__DIR__ . '/fixture_last_error_raise.php');
for ($i = 0; $i < 2000; $i++) {
    $spacers[] = sprintf('%036d', $i);
    $spacers[] = sprintf("%0{$fileLength}d", $i);
}

echo json_encode(oxphp_async_await(oxphp_async(static function (): array {
    $readAt = microtime(true);
    $error = error_get_last();

    return [
        'read_at' => $readAt,
        'seen' => $error === null ? null : [
            'type' => $error['type'],
            'message' => bin2hex(substr($error['message'], 0, 80)),
            'file' => bin2hex(substr($error['file'], 0, 80)),
        ],
    ];
})));
