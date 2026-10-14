<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Worker::current() can still be called once OxPHP has shut a request down: a
// session save handler registered with register_shutdown=false is written later
// in PHP's request shutdown, by the session extension. The instance the request
// got must have been released by then, the call must get a new instance rather
// than the released one — as must a destructor the release itself runs — and
// that instance must not reach the next request on the thread — see
// fixture_late.php.
//
// This test holds one of the profile's two workers, so both requests it sends
// run one after the other on the other worker.
$t = new TestCase('test_current_after_request_shutdown', 'worker_trad');

$out = '/tmp/worker-trad-' . bin2hex(random_bytes(6)) . '.json';

$get = static function (string $url): ?array {
    $ch = curl_init($url);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$late = $get('http://127.0.0.1/tests/worker_trad/fixture_late.php?out=' . rawurlencode($out));
$t->assertNotNull('the request with the late call answered', $late);
$t->assertSame('the WeakMap tracked the instance while the script ran', $late['tracked'] ?? null, 1);

// The next request on that thread starts only once the previous one has been
// shut down completely, so by the time it answers the write has run. Its pad
// moves the handle its own instance takes past the handles the first request
// gave out before its shutdown — see fixture_current.php.
$next = $get('http://127.0.0.1/tests/worker_trad/fixture_current.php?pad=3');
$t->assertNotNull('the next request answered', $next);
$t->assertTrue('the next request got an instance it created itself', $next['created_here'] ?? null);
$t->assertTrue('no two live objects in it share a handle', $next['distinct'] ?? null);
$t->assertTrue('its instance is still a Worker once another object is created', $next['is_worker'] ?? null);
$t->meta('next_ids', $next['ids'] ?? null);

$written = is_file($out) ? json_decode((string) file_get_contents($out), true) : null;
$t->assertNotNull('the save handler wrote from the session shutdown', $written);
$t->assertTrue(
    'the instance the script got was released before the session shutdown ran',
    $written['released_before'] ?? null
);
$t->assertType('Worker::current() still answered from inside the write', $written['late_id'] ?? null, 'integer');
$t->assertTrue(
    'the instance it answered with was a new one, not the released instance',
    is_int($written['script_id'] ?? null) && $written['script_id'] !== ($written['late_id'] ?? null)
);
$t->assertType(
    'a destructor ran while the instance was being released, and asked for one',
    $written['dtor_id'] ?? null,
    'integer'
);
$t->assertTrue(
    'it got a new instance, not the one being released',
    is_int($written['script_id'] ?? null) && $written['script_id'] !== ($written['dtor_id'] ?? null)
);
$t->meta('script_id', $written['script_id'] ?? null);
$t->meta('dtor_id', $written['dtor_id'] ?? null);
$t->meta('late_id', $written['late_id'] ?? null);

$t->assertTrue(
    'both requests and the write ran on the same worker, not this one',
    is_int($late['worker'] ?? null)
        && $late['worker'] !== oxphp_worker_id()
        && ($next['worker'] ?? null) === $late['worker']
        && ($written['worker'] ?? null) === $late['worker']
);

$t->done();
