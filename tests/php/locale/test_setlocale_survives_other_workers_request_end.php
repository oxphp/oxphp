<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// This request changes its LC_CTYPE, then a request on the other worker changes
// its own and ends. A response can reach the client before the request behind
// it has shut down, so a second request on that worker is the barrier: the
// worker takes it only once the first one has fully ended. The end of that
// request resets its own thread — the second one starts from the server's
// LC_CTYPE — and leaves this request's LC_CTYPE alone.
$t = new TestCase('test_setlocale_survives_other_workers_request_end', 'locale');

$fixture = static function (string $query): ?array {
    $ch = curl_init('http://127.0.0.1/tests/locale/fixture_locale.php?' . $query);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$base = setlocale(LC_CTYPE, '0');
$t->assertNotEqual('the server starts a request on an LC_CTYPE other than C', $base, 'C');
$t->assertSame('this request switches LC_CTYPE to C', setlocale(LC_CTYPE, 'C'), 'C');
$cEscaped = bin2hex(escapeshellarg("a\xffb"));

$changer = $fixture('set=LC_CTYPE');
$t->assertNotNull('the request that changes its locale answered', $changer);
$t->assertSame('it changed its LC_CTYPE to C', $changer['set'] ?? null, 'C');

$after = $fixture('');
$t->assertNotNull('the next request on that worker answered', $after);
$t->assertTrue(
    'both ran on the other worker, one after the other',
    is_int($changer['worker'] ?? null) && $changer['worker'] !== oxphp_worker_id()
        && ($after['worker'] ?? null) === $changer['worker']
);
$t->assertSame('the next request there starts from the server\'s LC_CTYPE', $after['ctype'] ?? null, $base);

$t->assertSame('this request still has the LC_CTYPE it set', setlocale(LC_CTYPE, '0'), 'C');
$t->assertSame('escapeshellarg() here still follows it', bin2hex(escapeshellarg("a\xffb")), $cEscaped);

$t->done();
