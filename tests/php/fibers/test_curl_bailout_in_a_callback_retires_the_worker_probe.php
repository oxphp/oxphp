<?php

declare(strict_types=1);

// Probe for fibers/test_curl_bailout_in_a_callback_retires_the_worker: the
// worker the transfer was ended on is gone, or for ?via=after is still here,
// and a handle this request can reach is one it can use.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/bailout_leak_probe.php';

$via = $_GET['via'] ?? 'easy';
$t = new TestCase('curl_bailout_in_a_callback_retires_the_worker_probe_' . $via, 'fibers');

if ($via === 'after') {
    OxphpBailoutLeak::assertKept($t, 'curl_' . $via);
} else {
    OxphpBailoutLeak::assertRetired($t, 'curl_' . $via);
}
if ($via === 'pause') {
    $t->assertSame('the callback failed inside curl_pause()', OxphpBailoutLeak::state('curl_pause')['failed_inside_curl_pause'] ?? null, true);
}
if ($via === 'remove' || $via === 'close') {
    $t->assertSame('the callback failed inside the handle\'s removal', OxphpBailoutLeak::state('curl_' . $via)['failed_inside_the_removal'] ?? null, true);
}

$h = $sharedState['curl_' . $via] ?? null;
if ($h instanceof \CurlHandle) {
    $t->assertSame('the handle the transfer left can be reset', fiber_attempt(static fn () => curl_reset($h)), 'no-throw');
    curl_setopt($h, CURLOPT_URL, 'http://127.0.0.1:9090/metrics');
    curl_setopt($h, CURLOPT_RETURNTRANSFER, true);
    $body = curl_exec($h);
    $t->assertTrue('and runs a transfer: ' . curl_error($h), is_string($body) && $body !== '');
    unset($sharedState['curl_' . $via]);
}

$t->done();
