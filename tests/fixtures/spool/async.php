<?php
// Output an async task writes goes to no client; it must stay off the spool.
$task = oxphp_async(function (): string {
    echo str_repeat('a', 2 << 20);
    return 'done';
});
echo oxphp_async_await($task, 10.0);
