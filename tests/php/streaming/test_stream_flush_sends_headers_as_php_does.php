<?php

declare(strict_types=1);

require_once __DIR__ . '/stream_probe.php';

// Runner-side test: a stream whose first flush sends its headers before any
// output tells PHP so, as PHP-FPM's flush does. headers_sent() turns true, a
// header() after that warns and goes nowhere, and the engine's default
// Content-Type and a header_register_callback() callback make it into the
// headers that go out. The runner checks the headers; the body is the test's
// own report.

header_register_callback(static function (): void {
    header('X-Cb: ran');
});

oxphp_stream_flush();
$sent = headers_sent();

$warnings = [];
set_error_handler(static function (int $errno, string $message) use (&$warnings): bool {
    $warnings[] = $message;

    return true;
}, E_WARNING);
header('X-Late: 1');
restore_error_handler();

stream_probe_report('stream_flush_sends_headers_as_php_does', [
    'headers_sent() once the stream has sent its headers' => $sent === true,
    'a header() after that warns that they have gone' => count(array_filter(
        $warnings,
        static fn (string $m): bool => str_contains($m, 'headers already sent'),
    )) === 1,
]);
oxphp_stream_flush();
