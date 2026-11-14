<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request on the other worker switches LC_CTYPE to C and leaves its session
// for the session module to write at shutdown. PHP puts the locale back before
// that write, so the save handler runs under the server's LC_CTYPE. The
// LC_CTYPE the handler then sets must not reach the next request on that
// worker, which is also the barrier: the worker takes it only once the first
// request has fully ended.
$t = new TestCase('test_setlocale_reset_before_session_shutdown_write', 'locale');

$get = static function (string $path): ?array {
    $ch = curl_init('http://127.0.0.1/tests/locale/' . $path);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$base = setlocale(LC_CTYPE, '0');
$t->assertNotEqual('the server starts a request on an LC_CTYPE other than C', $base, 'C');

$token = bin2hex(random_bytes(8));
$record = sys_get_temp_dir() . "/oxphp-locale-session-$token.json";

$writer = $get('fixture_locale_session.php?token=' . $token);
$t->assertNotNull('the request with the open session answered', $writer);
$t->assertSame('it changed its LC_CTYPE to C', $writer['set'] ?? null, 'C');

$after = $get('fixture_locale.php');
$t->assertNotNull('the next request on that worker answered', $after);
$t->assertTrue(
    'both ran on the other worker, one after the other',
    is_int($writer['worker'] ?? null) && $writer['worker'] !== oxphp_worker_id()
        && ($after['worker'] ?? null) === $writer['worker']
);

$written = is_file($record) ? json_decode((string) file_get_contents($record), true) : null;
@unlink($record);
$t->assertTrue('the save handler wrote the session at shutdown', is_array($written));
$t->assertSame(
    'it wrote it under the server\'s LC_CTYPE, as with PHP',
    is_array($written) ? ($written['ctype'] ?? null) : null,
    $base
);
$t->assertSame(
    'the LC_CTYPE the save handler set does not reach the next request there',
    $after['ctype'] ?? null,
    $base
);

$t->done();
