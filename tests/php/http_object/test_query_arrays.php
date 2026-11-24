<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_arrays', 'http_object');

// Runs on the default profile and on both SUPERGLOBALS_ENABLED=false profiles.
// The expected array is what parse_str() and $_GET make of the query string the
// suite sends: bracketed names nest, `[]` appends, a scalar followed by `k[]`
// becomes an array, a name with no base before `[` is dropped, and `.`, a space
// and an unclosed `[` turn into `_`.
$expected = [
    'a'      => ['1', '2'],
    'f'      => ['s' => ['t' => 'x']],
    'a_b'    => '1',
    'k'      => ['2'],
    'n'      => [2 => 'x', 3 => 'y'],
    'sp_ace' => '1',
    'd_'     => '1',
    'm_x'    => '1',
];

$req = oxphp_http_request();
$all = $req->query();

$t->assertSame('query() is the array parse_str() makes', $all, $expected);
$t->assertSame("query('a') is the list", $req->query('a'), ['1', '2']);
$t->assertSame("query('f') is the nested array", $req->query('f'), ['s' => ['t' => 'x']]);
$t->assertSame("query('a_b') reads the mangled name", $req->query('a_b'), '1');
$t->assertNull("query('a.b') finds nothing, as \$_GET does not", $req->query('a.b'));
$t->assertNull("query('a[]') finds nothing", $req->query('a[]'));
$t->assertSame('a missing key falls back to the default', $req->query('none', ['d']), ['d']);
$t->assertSame('query() is the same array on a second call', $req->query(), $all);

if (oxphp_superglobals_enabled()) {
    $t->assertSame('query() === $_GET', $all, $_GET);
} else {
    $t->assertEmpty('$_GET stays empty with the superglobals off', $_GET);
}

$t->done();
