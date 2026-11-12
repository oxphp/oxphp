<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A Request object belongs to the request it was built for.
//
// A worker keeps whatever PHP state the application keeps, so a Request object
// stored in a static, a container or a closure outlives its request. Answering
// there, its methods would read whatever request the thread is serving at the
// call, and the ones that cache, payload() and query() without a key, the request
// they were first called in: one client's data handed to the code serving
// another, with nothing to say so. Every method throws instead.
//
// PHP_WORKERS=1, so the inner self-request can only be served while this request
// is parked on its read, on the same thread. The inner request differs in every
// value the calls read (query, cookie, body), so an answer that leaks shows as
// the other request's value rather than as an empty one.

use OxPHP\Http\Request;

$t = new TestCase('request_object_refuses_another_request', 'hooks');

$calls = require __DIR__ . '/request_object_calls.php';
$foreign = 'OxPHP\Http\Exception\NoActiveRequestException: '
    . 'Cannot access HTTP request: this Request object belongs to another request';

// What a call did: the exception, or what it returned, so a leak shows the
// other request's value.
$attempt = static function (callable $fn): string {
    try {
        return 'no-throw: ' . json_encode($fn());
    } catch (\Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
};

// The list has to be the whole class, or a method added later goes unchecked.
$methods = get_class_methods(Request::class);
sort($methods);
$listed = array_keys($calls);
sort($listed);
$t->assertSame('the call list covers every method of Request', $listed, $methods);

$own = oxphp_http_request();
$t->assertSame('outer request reads its own body', $own->payload('who'), 'outer');
$t->assertSame('outer request reads its own query', $own->query()['who'] ?? null, 'outer');
$own->attributes()->set('owner', 'outer');

unset($sharedState['inner_request']);
$sharedState['held_request'] = $own;
try {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 5.0);
    $t->assertTrue('inner self-request socket connected', $sock !== false);
    stream_set_timeout($sock, 5);

    $innerBody = '{"who":"inner"}';
    fwrite($sock, "POST /tests/hooks/fixture_held_request.php?who=inner HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\n"
        . "Cookie: ox_who=inner\r\n"
        . "Content-Type: application/json\r\n"
        . "Content-Length: " . strlen($innerBody) . "\r\n"
        . "Connection: close\r\n\r\n"
        . $innerBody);

    $raw = (string) stream_get_contents($sock);     // hooked: parks this request
    fclose($sock);
} finally {
    unset($sharedState['held_request']);
}

$split = strpos($raw, "\r\n\r\n");
$inner = json_decode($split === false ? '' : substr($raw, $split + 4), true);

// Without these the rest proves nothing: the inner request has to have run on
// this thread, with a request of its own, and found the outer object.
$t->assertTrue('inner request answered with JSON', is_array($inner));
$t->assertSame('inner request found the outer object', $inner['held'] ?? null, true);
$t->assertSame('inner request reads its own body', $inner['own_who'] ?? null, 'inner');
$t->assertSame('inner request reads its own query', $inner['own_query'] ?? null, 'inner');
$t->assertSame('inner request\'s own object answers in its output callback',
    $inner['ob_who'] ?? null, 'inner');

foreach (array_keys($calls) as $name) {
    $t->assertSame("$name() on the outer object throws inside the inner request",
        $inner['results'][$name] ?? 'missing', $foreign);
}

// The other half: the object of the request being served keeps answering, with
// this request's values, after the inner request has come and gone.
foreach ($calls as $name => $call) {
    $t->assertMatch("$name() on the outer request's own object still answers",
        $attempt(static fn() => $call($own)), '/^no-throw: /');
}
$t->assertSame('payload() still reads the outer body', $own->payload('who'), 'outer');
$t->assertSame('query() still reads the outer query', $own->query('who'), 'outer');
$t->assertSame('query() keeps the outer array', $own->query()['who'] ?? null, 'outer');
$t->assertSame('method() reads the outer method', $own->method(), 'POST');
$t->assertSame('attributes() keeps the outer container', $own->attributes()->get('owner'), 'outer');

// The inner request has ended and left its object behind with its caches filled.
// Here that object would hand the inner client's body to this request.
$ended = $sharedState['inner_request'] ?? null;
unset($sharedState['inner_request']);
$t->assertInstanceOf('inner request left its object behind', $ended, Request::class);
if ($ended instanceof Request) {
    $t->assertSame('payload() on an ended request\'s object throws',
        $attempt(static fn() => $ended->payload('who')), $foreign);
    $t->assertSame('query() on an ended request\'s object throws',
        $attempt(static fn() => $ended->query()), $foreign);
    $t->assertSame('attributes() on an ended request\'s object throws',
        $attempt(static fn() => $ended->attributes()), $foreign);
    $t->assertSame('method() on an ended request\'s object throws',
        $attempt(static fn() => $ended->method()), $foreign);
}

// A thread serving no request at all: an object built on an async worker.
$onTask = oxphp_async_await(oxphp_async(static function (): string {
    try {
        return 'no-throw: ' . json_encode((new \OxPHP\Http\Request())->method());
    } catch (\Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
}), 5.0);
$t->assertSame('a Request built on an async worker throws', $onTask,
    'OxPHP\Http\Exception\AsyncContextException: Cannot access HTTP request from async worker context');

// An object built in the worker script before oxphp_worker() belongs to the boot.
$boot = $sharedState['boot_request'] ?? null;
$t->assertInstanceOf('the worker boot left its object', $boot, Request::class);
if ($boot instanceof Request) {
    $t->assertSame('method() on the boot\'s object throws inside a request',
        $attempt(static fn() => $boot->method()), $foreign);
}

$t->done();
