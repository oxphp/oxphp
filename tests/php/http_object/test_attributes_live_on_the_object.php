<?php
// The attributes container belongs to the Request object it was taken from, not
// to the request. oxphp_http_request() ends in a bare object_init_ex — it builds
// a new Request every time it is called — and attributes() caches its container
// in a property of that object. So a write through one call is invisible to a
// read through another, silently, with the default coming back in its place.
// What makes a container shared is passing the object along.
//
// Both halves are pinned here because the documentation describes the container
// as per-request state that is reset between requests, which would make the
// first half of this test fail and the second half unnecessary.

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('attributes_live_on_the_object', 'http_object');

// Two calls, two objects.
$a = oxphp_http_request();
$b = oxphp_http_request();
$t->assertTrue('two oxphp_http_request() calls return two objects', $a !== $b);

// One object hands back one container, on every call.
$t->assertTrue(
    'attributes() is cached on the object it was called on',
    $a->attributes() === $a->attributes()
);

// Two objects, two containers — and the write is not visible through the second
// by any of the three ways of asking.
$a->attributes()->set('tenant_id', 'acme');
$t->assertTrue(
    'a second Request carries a container of its own',
    $a->attributes() !== $b->attributes()
);
$t->assertNull(
    'a write through one call reads back as the default through another',
    $b->attributes()->get('tenant_id')
);
$t->assertFalse('has() does not see it either', $b->attributes()->has('tenant_id'));
$t->assertSame('the second container is empty', $b->attributes()->all(), []);

// Pass the object along and there is one container. This is the form that works.
$enrich = static function (\OxPHP\Http\RequestInterface $r): void {
    $r->attributes()->set('locale', 'ru_RU');
};
$enrich($a);
$t->assertSame(
    'a value written through the object that was passed along reads back',
    $a->attributes()->get('locale'),
    'ru_RU'
);

// A Fiber is not a special case of either half: handed the object it writes into
// the same container, and calling oxphp_http_request() itself it gets its own.
$fiber = new Fiber(static function (\OxPHP\Http\RequestInterface $r) use ($t): void {
    $r->attributes()->set('from_fiber', 1);
    $t->assertNull(
        'a Fiber calling oxphp_http_request() itself gets an empty container',
        oxphp_http_request()->attributes()->get('locale')
    );
});
$fiber->start($a);
$t->assertSame(
    'a Fiber given the object writes into that object\'s container',
    $a->attributes()->get('from_fiber'),
    1
);

$t->done();
