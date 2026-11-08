<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Reading the body must not show up in the state json_last_error() reports, in
// either direction, whatever the body is. Each direction takes a new object,
// because only the first read on an object parses.
$t = new TestCase('payload_json_last_error', 'http_object');

json_decode('{bad');
$t->assertSame('an error is standing', json_last_error(), JSON_ERROR_SYNTAX);
oxphp_http_request()->payload();
$t->assertSame('payload() does not clear it', json_last_error(), JSON_ERROR_SYNTAX);

json_decode('1');
$t->assertSame('no error is standing', json_last_error(), JSON_ERROR_NONE);
oxphp_http_request()->payload();
$t->assertSame('payload() does not set one', json_last_error(), JSON_ERROR_NONE);

$t->done();
