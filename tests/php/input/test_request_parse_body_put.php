<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() is how a PUT body gets parsed: nothing parses it before
// the script runs. OxPHP wraps the function in every mode — in the classic mode
// it passes every call through, and in Worker mode a first call on a PUT has no
// earlier parse to keep — and the call must return what PHP's own parse does.

$t = new TestCase('request_parse_body_put', 'input');

[$post, $files] = request_parse_body();

$t->assertSame('the urlencoded PUT body is parsed', $post, ['probe' => 'put', 'n' => '2']);
$t->assertSame('and carries no files', $files, []);

$t->done();
