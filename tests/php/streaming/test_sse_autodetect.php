<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('sse_autodetect', 'streaming');

header('Content-Type: text/event-stream');
$t->assertTrue('oxphp_is_streaming() === true after text/event-stream header', oxphp_is_streaming() === true);

$t->done();
