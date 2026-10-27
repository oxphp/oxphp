<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A fatal from inside a request_parse_body() call that has already put its own
// Content-Type copy in place of the one standing must still free the one it
// replaced. The error handler runs for a body over post_max_size, which the
// reader reports after the copy is replaced. The header is over 100 KB so that
// a copy lost to the fatal shows in what the worker keeps from one request to
// the next.
//
// Three identical sightings on one worker (PHP_WORKERS=1 in this profile), of a
// type PHP has no parser for, whose copy nothing consumes:
//   1. records the worker's memory as the script starts;
//   2. fatals from the handler inside the call;
//   3. records it again and compares.
//
// The fatal redeclares a class rather than calling engine_fatal(): a compile
// error leaves about 39 KB of the compiler's arena on a worker, too close to
// what is measured here. Declared conditionally because the worker keeps its
// function table across requests.

if (!function_exists('oxphp_rpb_copy_fatal')) {
    function oxphp_rpb_copy_fatal(): void
    {
        class OxphpRpbCopyFatal
        {
        }
    }
}

$memory = memory_get_usage();

$t = new TestCase('request_parse_body_bailout_frees_replaced_type_copy', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);
$t->assertGreaterThan(
    'the content type is long enough to measure',
    strlen((string) ($_SERVER['CONTENT_TYPE'] ?? '')),
    100000
);

if ($sighting === 1) {
    $sharedState['request_parse_body_copy_memory'] = $memory;
    $t->assertTrue('recorded the memory before the fatal', true);
} elseif ($sighting === 2) {
    // Disarmed before it fatals: the worker keeps the handler, which the fatal
    // leaves on the stack, and from then on it passes every error through.
    $sharedState['request_parse_body_copy_fatal_armed'] = true;
    set_error_handler(static function () use (&$sharedState): bool {
        if (($sharedState['request_parse_body_copy_fatal_armed'] ?? false) !== true) {
            return false;
        }
        $sharedState['request_parse_body_copy_fatal_armed'] = false;
        oxphp_rpb_copy_fatal();
        oxphp_rpb_copy_fatal();
        return true;
    });
    request_parse_body(['post_max_size' => 1]);
    restore_error_handler();
    $t->assertTrue('unreachable: the handler fatals inside the call', false);
} else {
    $before = $sharedState['request_parse_body_copy_memory'] ?? null;
    $armed = $sharedState['request_parse_body_copy_fatal_armed'] ?? null;
    unset($sharedState['request_parse_body_copy_memory'], $sharedState['request_parse_body_copy_fatal_armed']);

    $t->assertSame('the request before this one fatalled from inside the call', $armed, false);
    $t->assertType('the first sighting recorded its memory', $before, 'integer');
    $grown = $memory - (int) $before;
    $t->meta('memory_grown_across_the_fatal', $grown);
    $t->assertLessThan('the copy the fatal call had replaced was freed', $grown, 50000);
}

$t->done();
