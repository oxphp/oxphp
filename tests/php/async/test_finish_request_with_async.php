<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('finish_request_with_async', 'async');

// Dispatch before finish_request
$p = oxphp_async(function(): string {
    usleep(100000); // 100ms background work
    return 'background_done';
});

$t->assertTrue('async dispatch before finish_request succeeded', is_int($p));

// Output test JSON before finish_request closes the response
$t->done();

// Send HTTP response early — connection closed after this
oxphp_finish_request();

// Background: await still works after finish_request (not reportable to client)
$result = oxphp_async_await($p);
