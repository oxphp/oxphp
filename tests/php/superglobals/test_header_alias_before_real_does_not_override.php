<?php
// Sent: `X.Forwarded.For: 6.6.6.6`, then `X-Forwarded-For: 9.9.9.9` — the
// order in which the real header already won by coming last. Kept next to the
// opposite order so that a fix which only changes who wins is caught too.
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_alias_before_real_does_not_override', 'superglobals');
$t->assertSame('HTTP_X_FORWARDED_FOR is the hyphenated header', $_SERVER['HTTP_X_FORWARDED_FOR'] ?? null, '9.9.9.9');
$t->done();
