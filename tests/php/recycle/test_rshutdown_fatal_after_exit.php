<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The worker that left raised a fatal error inside the session extension's
// request shutdown on its way out (see test_rshutdown_fatal_arm). From PHP
// 8.4.28 / 8.5.13 PHP ends the process there unless the server catches it, and
// this request would find no server to answer it.

$test = new TestCase('rshutdown_fatal_after_exit', 'recycle');

$tmp = sys_get_temp_dir();
$test->assertFalse(
    'the session started after the loop',
    is_file("$tmp/oxphp-rshutdown-fatal-worker.nosession")
);
$test->assertTrue(
    'the stream was closed, and its stream_close() raised the fatal',
    is_file("$tmp/oxphp-rshutdown-fatal-worker.closed")
);
if (is_file("$tmp/oxphp-rshutdown-fatal-worker.closed")) {
    unlink("$tmp/oxphp-rshutdown-fatal-worker.closed");
}

$test->assertSame(
    'a freshly booted worker is serving',
    OxPHP\Server\Worker::current()->requestCount(),
    1
);

$test->done();
