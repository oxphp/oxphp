<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('is_streaming_false', 'sapi');

$t->assertFalse('oxphp_is_streaming() === false before any flush', oxphp_is_streaming());

$t->done();
