<?php
// Runner-side test: validates header_remove("Set-Cookie", "PHPSESSID=") only
// removes the matching prefixed cookie, leaving other Set-Cookie headers
// intact. PHP 8.5 dispatches this via SAPI_HEADER_DELETE_PREFIX; PHP 8.4
// ignores the prefix arg entirely (no-op), so we cannot observe the new
// semantic on 8.4 and the test trivially passes there.
//
// TestCase has no skip() helper at the time of writing, so on 8.4 we just
// call $t->done() with no assertions registered (vacuous PASS). On 8.5 the
// real assertions run; without a SAPI_HEADER_DELETE_PREFIX handler arm in
// oxphp_header_handler the PHPSESSID cookie stays in headers_list() and
// assertNotContains fails.

require __DIR__ . '/../test_helper.php';
$t = new TestCase('header_remove_prefix', 'headers');

if (PHP_VERSION_ID < 80500) {
    // 8.4 ignores the prefix arg in header_remove(), so this test cannot
    // observe the 8.5 semantic. Record a vacuous PASS on 8.4.
    $t->done();
    return;
}

header('Set-Cookie: PHPSESSID=abc123; Path=/');
header('Set-Cookie: keep_me=value; Path=/', false);
header_remove('Set-Cookie', 'PHPSESSID=');

$headers = implode("\n", headers_list());
$t->assertContains('keep_me cookie retained', $headers, 'keep_me=value');
$t->assertNotContains('PHPSESSID cookie removed', $headers, 'PHPSESSID');
$t->done();
