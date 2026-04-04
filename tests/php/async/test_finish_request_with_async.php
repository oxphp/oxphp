<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('finish_request_with_async', 'async');

// Dispatch before finish_request
$p = oxphp_async(function(): string {
    usleep(100000); // 100ms background work
    return 'background_done';
});

// Send HTTP response early
oxphp_finish_request();

// Client received response; we can still await
$result = oxphp_async_await($p);
$t->assertSame('async task completed after finish_request', $result, 'background_done');

$t->done();
