<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Runs right after fixture_rshutdown_fatal.php, whose request ended in a fatal
// error inside the session's request shutdown, on a profile with one PHP
// worker thread.
//
// The globals of that thread's session extension may still point into the
// request heap it has since freed, so the thread must not serve this request:
// the server retires it and starts another. requestCount() counts the requests
// a thread has started, so 1 here means a thread that started after the
// fatal; the thread that took it had served at least that request. A server
// that lets the thread serve on reports more, and one that lets PHP end the
// process does not answer at all.

$t = new TestCase('rshutdown_fatal_retires_thread', 'bailout');

$marker = '/tmp/oxphp-rshutdown-fatal-marker';
$t->assertTrue('the request before raised its fatal while closing its stream', is_file($marker));
@unlink($marker);

$t->assertSame(
    'this request runs on a thread started after the fatal',
    \OxPHP\Server\Worker::current()->requestCount(),
    1
);

$t->done();
