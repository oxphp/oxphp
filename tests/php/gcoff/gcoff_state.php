<?php

declare(strict_types=1);

// Where the trigger of the gcoff profile leaves the worker it ran on and the
// collector's guard as it stood before the fatal, for the requests after it to
// compare with their own.

if (!function_exists('oxphp_gcoff_record_worker')) {
    function oxphp_gcoff_record_worker(): void
    {
        // JSON, not a string cast, which rounds to the `precision` directive.
        file_put_contents('/tmp/oxphp-gcoff-worker', json_encode([
            'worker' => OxPHP\Server\Worker::current()->startTime(),
            'protected' => gc_status()['protected'],
        ]));
    }

    function oxphp_gcoff_recorded(string $key): mixed
    {
        $raw = @file_get_contents('/tmp/oxphp-gcoff-worker');
        $state = is_string($raw) ? json_decode($raw, true) : null;

        return is_array($state) ? ($state[$key] ?? null) : null;
    }

    function oxphp_gcoff_recorded_worker(): ?float
    {
        $time = oxphp_gcoff_recorded('worker');

        return is_float($time) ? $time : null;
    }
}
