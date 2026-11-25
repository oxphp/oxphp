<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_cache_slot_written_from_userland', 'http_object');
// array_walk() hands the script a reference to the slot query() caches its
// array in, and the script can tie that reference to code of its own. The
// suite passes the case in the query string, read from $_GET so the cache
// stays empty until the call under test. PHP 8.6 deprecates passing an
// object to array_walk() but still walks it.
$req = oxphp_http_request();

if (($_GET['case'] ?? '') === 'typed') {
    // A ?int property bound to the reference refuses the array query() stores.
    $holder = new class {
        public ?int $slot = null;
    };
    ignoring_deprecations(static fn () => array_walk($req, static function (&$v, $k) use ($holder) {
        if (str_ends_with($k, '_query_cache')) {
            $holder->slot = &$v;
        }
    }));
    $t->assertThrows("query('a') throws the TypeError of the refused store", static fn () => $req->query('a'), \TypeError::class);
    $t->assertNull('the bound property keeps its value', $holder->slot);
} else {
    // What the slot holds is destroyed as query() stores the array over it,
    // and its destructor writes the slot again.
    $GLOBALS['destructed'] = false;
    ignoring_deprecations(static fn () => array_walk($req, static function (&$v, $k) {
        if (str_ends_with($k, '_query_cache')) {
            $rewriter = new class {
                public $slot;

                public function __destruct()
                {
                    $GLOBALS['destructed'] = true;
                    $this->slot = 5;
                }
            };
            $rewriter->slot = &$v;
            $v = $rewriter;
        }
    }));
    $t->assertFalse('the destructor has not run before query()', $GLOBALS['destructed']);
    $t->assertSame("query('a') answers from the array it parsed", $req->query('a'), ['1', '2']);
    $t->assertTrue('the destructor ran inside that call', $GLOBALS['destructed']);
    $t->assertSame("query('a') parses again over the 5 the destructor left", $req->query('a'), ['1', '2']);
}
$t->done();
