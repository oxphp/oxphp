<?php
// Sent: `X-Forwarded-For: 9.9.9.9`, then `X.Forwarded.For: 6.6.6.6`.
// A later key overwrites an earlier one when PHP registers $_SERVER, so the
// alias sent after the real header would replace its value.
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_alias_after_real_does_not_override', 'superglobals');
$t->assertSame('HTTP_X_FORWARDED_FOR is the hyphenated header', $_SERVER['HTTP_X_FORWARDED_FOR'] ?? null, '9.9.9.9');
$t->done();
