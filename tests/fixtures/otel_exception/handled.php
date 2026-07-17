<?php
// Negative case: the app installs its own exception handler and renders its own
// 500. The exception is consumed before it becomes uncaught, so OxPHP never sees
// a Throwable — no exception event must appear on the span.
set_exception_handler(function (\Throwable $e) {
    http_response_code(500);
    echo 'handled by app';
});
throw new RuntimeException('handled path: should not appear on span');
