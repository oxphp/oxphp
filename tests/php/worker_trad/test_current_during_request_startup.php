<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Worker::current() can be called before OxPHP's module has started a request:
// with session.auto_start, PHP decodes the session — and wakes the objects in
// it — while the session extension starts the request, and that extension
// starts before OxPHP's. Whatever an earlier request on the thread left behind
// must be gone by then — including an instance asked for after OxPHP had shut
// that request down, which nothing released.
//
// This test holds one of the profile's two workers, so both requests it sends
// run one after the other on the other worker. See fixture_autostart_seed.php
// and fixture_autostart_probe.php.
$t = new TestCase('test_current_during_request_startup', 'worker_trad');

$out = '/tmp/worker-trad-auto-' . bin2hex(random_bytes(6)) . '.json';

$get = static function (string $url, ?string $cookie = null): ?array {
    $ch = curl_init($url);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    if ($cookie !== null) {
        curl_setopt($ch, CURLOPT_COOKIE, $cookie);
    }
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$seed = $get('http://127.0.0.1/tests/worker_trad/fixture_autostart_seed.php?out=' . rawurlencode($out));
$t->assertNotNull('the seeding request answered', $seed);
$sid = $seed['sid'] ?? null;
$t->assertType('it opened a session', $sid, 'string');

$probe = $get(
    'http://127.0.0.1/tests/worker_trad/fixture_autostart_probe.php',
    'PHPSESSID=' . (is_string($sid) ? $sid : '')
);
$t->assertNotNull('the next request answered', $probe);
$t->assertTrue('its session woke the probe up, and the probe got an instance', $probe['woken'] ?? null);
$t->assertTrue(
    'that instance was created by this request, right after the probe',
    $probe['created_here'] ?? null
);
$t->assertTrue('the script got the same instance', $probe['same'] ?? null);
$t->assertTrue('no two live objects in it share a handle', $probe['distinct'] ?? null);
$t->assertTrue('its instance is still a Worker once another object is created', $probe['is_worker'] ?? null);
$t->meta('ids', $probe['ids'] ?? null);

$written = is_file($out) ? json_decode((string) file_get_contents($out), true) : null;
$t->assertType(
    'the seeding request asked for an instance after OxPHP had shut it down',
    $written['late_id'] ?? null,
    'integer'
);
$t->meta('late_id', $written['late_id'] ?? null);

$t->assertTrue(
    'both requests and the write ran on the same worker, not this one',
    is_int($seed['worker'] ?? null)
        && $seed['worker'] !== oxphp_worker_id()
        && ($probe['worker'] ?? null) === $seed['worker']
        && ($written['worker'] ?? null) === $seed['worker']
);

$t->done();
