<?php

declare(strict_types=1);

// Fetched by test_task_last_error_does_not_outlive_its_request. The warning a
// task raises here names a literal of this request as its message and this
// file as its file, both in memory the end of this request frees.
echo json_encode(oxphp_async_await(oxphp_async(static function (): array {
    @trigger_error('oxphp-last-error-of-an-ended-request', E_USER_WARNING);
    $error = error_get_last();
    ob_start();
    debug_zval_dump($error['file']);

    return ['message' => $error['message'], 'file' => ob_get_clean()];
})));
