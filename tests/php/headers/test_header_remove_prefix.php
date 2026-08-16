<?php
// Runner-side test: validates that header_remove("Set-Cookie", "PHPSESSID=")
// only removes the matching prefixed cookie, leaving other Set-Cookie headers
// intact.
//
// PHP 8.5 introduced this 2-arg form of header_remove() and dispatches it
// through SAPI_HEADER_DELETE_PREFIX. The OxPHP SAPI handler for that op
// is wired in src/php/sapi.rs (cfg-gated under php_v8_5).
//
// Compatibility:
//   - PHP 8.4: header_remove() is 1-arg; the prefix arg silently ignored.
//   - PHP 8.5.0–8.5.5: SAPI op exists at C level but the userland 2-arg
//     signature isn't exposed yet (lands in 8.5.6+).
//   - PHP 8.5.6+: 2-arg userland form available; this test exercises it.
//
// We use reflection to probe for the 2-arg form rather than hard-coding
// a PHP_VERSION_ID < 80506 check — robust to any patch where the
// signature first lands.

require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_remove_prefix', 'headers');

$rf = new ReflectionFunction('header_remove');
if ($rf->getNumberOfParameters() < 2) {
    // 2-arg form not exposed in this PHP build. The op may still exist at
    // the C SAPI level (8.5.0–8.5.5) but is not invokable from PHP, so
    // there's nothing to assert here. Record vacuous PASS.
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
