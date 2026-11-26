<?php

declare(strict_types=1);

// Helpers for the streaming/ tests: a client that fetches a fixture from this
// server as it arrives, a marker file shared by a test and its inner request,
// and the report a test writes when TestCase cannot be used — its done() sets a
// header, which a stream that has sent its headers refuses.

/** The marker file a test and its inner request share under ?token=. */
function stream_probe_marker(string $token): string
{
    return sys_get_temp_dir() . '/oxphp-stream-probe-' . preg_replace('/[^0-9a-f]/', '', $token);
}

/**
 * Fetches $path from this server over HTTP/1.1 or, over cleartext, HTTP/2,
 * handing the body received so far to $onBody each time more of it arrives.
 *
 * @return array{status: int, headers: array<string, string>, body: string, error: string}
 */
function stream_probe_fetch(string $path, int $httpVersion, ?callable $onBody = null): array
{
    $headers = [];
    $body = '';
    $ch = curl_init('http://127.0.0.1' . $path);
    curl_setopt_array($ch, [
        CURLOPT_HTTP_VERSION => $httpVersion,
        CURLOPT_TIMEOUT => 15,
        CURLOPT_HEADERFUNCTION => static function ($ch, string $line) use (&$headers): int {
            $colon = strpos($line, ':');
            if ($colon !== false) {
                $headers[strtolower(trim(substr($line, 0, $colon)))] = trim(substr($line, $colon + 1));
            }

            return strlen($line);
        },
        CURLOPT_WRITEFUNCTION => static function ($ch, string $data) use (&$body, $onBody): int {
            $body .= $data;
            if ($onBody !== null) {
                $onBody($body);
            }

            return strlen($data);
        },
    ]);
    curl_exec($ch);

    return [
        'status' => (int) curl_getinfo($ch, CURLINFO_RESPONSE_CODE),
        'headers' => $headers,
        'body' => $body,
        'error' => curl_error($ch),
    ];
}

/**
 * Waits up to $seconds for $file to exist, parking the request between looks
 * where it can be parked. Whether it appeared.
 */
function stream_probe_wait_for(string $file, float $seconds): bool
{
    $deadline = microtime(true) + $seconds;
    do {
        clearstatcache(true, $file);
        if (is_file($file)) {
            return true;
        }
        oxphp_usleep(10_000);
    } while (microtime(true) < $deadline);

    return false;
}

/**
 * Writes a test's report in the shape TestCase::done() writes it, without
 * setting a header.
 *
 * @param array<string, bool> $checks
 */
function stream_probe_report(string $test, array $checks): void
{
    $assertions = [];
    foreach ($checks as $name => $pass) {
        $assertions[] = ['name' => $name, 'pass' => $pass];
    }
    echo json_encode([
        'test' => $test,
        'group' => 'streaming',
        'pass' => !in_array(false, $checks, true),
        'assertions' => $assertions,
        'error' => null,
        'meta' => new stdClass(),
    ], JSON_UNESCAPED_SLASHES);
}
