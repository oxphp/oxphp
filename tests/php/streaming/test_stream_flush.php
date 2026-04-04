<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_flush', 'streaming');

oxphp_stream_flush();
$t->assertTrue('oxphp_is_streaming() === true after oxphp_stream_flush()', oxphp_is_streaming() === true);

$t->done();
