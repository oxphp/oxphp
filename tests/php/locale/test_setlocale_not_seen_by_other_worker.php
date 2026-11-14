<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// This request changes its LC_CTYPE and, while still running, asks the server
// for another request — which lands on the other worker. That request keeps the
// LC_CTYPE the server started it with: setlocale() is per request, not per
// process.
$t = new TestCase('test_setlocale_not_seen_by_other_worker', 'locale');

$fixture = static function (string $query): ?array {
    $ch = curl_init('http://127.0.0.1/tests/locale/fixture_locale.php?' . $query);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$base = setlocale(LC_CTYPE, '0');
$baseEscaped = bin2hex(escapeshellarg("a\xffb"));
$t->assertNotEqual('the server starts a request on an LC_CTYPE other than C', $base, 'C');

$t->assertSame('this request switches LC_CTYPE to C', setlocale(LC_CTYPE, 'C'), 'C');
$cEscaped = bin2hex(escapeshellarg("a\xffb"));
$t->assertNotEqual('escapeshellarg() here follows the change', $cEscaped, $baseEscaped);

$other = $fixture('');
$t->assertNotNull('the other request answered', $other);
$t->assertTrue(
    'the other request ran on the other worker, while this one is running',
    is_int($other['worker'] ?? null) && $other['worker'] !== oxphp_worker_id()
);
$t->assertSame('the other request still has the server\'s LC_CTYPE', $other['ctype'] ?? null, $base);
$t->assertSame('escapeshellarg() there still follows it', $other['escaped'] ?? null, $baseEscaped);

$t->assertSame('this request keeps its own LC_CTYPE', setlocale(LC_CTYPE, '0'), 'C');

$t->done();
