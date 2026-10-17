<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Counter;
use OxPHP\Shared\Registry;

// The last reference to a Shared\Pool can go away on the thread that parked
// slots in it, with its request open: the pool's teardown then runs $destroy on
// those slots. A fatal inside $destroy has to end the request as it would
// anywhere else, and what the teardown and the code that let go of the pool
// were doing has to be finished on the fatal's way out: the pool leaves the
// shared registry, the slots $destroy did not reach go without it, and what
// that code held is given back — the lock Map::clear() holds while it lets go
// of the values. Weak references to the pool let go of it before $destroy runs.
//
// The fixture's pools park two slots, and $destroy fatals on the first one the
// teardown reaches.

$t = new TestCase('shared_pool_drop_fatal', 'fibers');

$run = bin2hex(random_bytes(4));

/** One request to this server, read to its end. */
$get = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: $errstr";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_shared_closure_fatal.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked (this profile hooks streams): the read parks this request's fiber,
    // which leaves the worker free to serve the request just sent.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

/**
 * Entries the shared registry still counts but no longer lists: an entry is
 * listed while something holds it, and counted until its teardown has run to
 * its end. Null when the summary cannot be read.
 */
$unfinished = static function (): ?int {
    $summary = json_decode((string) file_get_contents('http://127.0.0.1:9090/__ox_shared/summary'), true);
    if (!is_int($summary['total_entries'] ?? null)) {
        return null;
    }
    $listed = 0;
    foreach ($summary['by_type'] ?? [] as $type) {
        $listed += $type['count'];
    }

    return $summary['total_entries'] - $listed;
};

/** Pool slots let go of without their $destroy because their request ended. */
$skipped = static function (): int {
    $metrics = (string) file_get_contents('http://127.0.0.1:9090/metrics');

    return preg_match('/^oxphp_shared_pool_destroy_skipped_total\{reason="request_ended"\} (\d+)$/m', $metrics, $m)
        ? (int) $m[1]
        : -1;
};

foreach (['unset', 'weak', 'map_remove', 'map_clear'] as $how) {
    $unfinishedBefore = $unfinished();
    $skippedBefore = $skipped();

    $resp = $get("case=pool_drop_$how&run=$run");
    $t->assertMatch("$how: the fatal ended the inner request with a 500", $resp, '#^HTTP/\S+ 500 #');
    $t->assertNotContains("$how: nothing past the fatal ran", $resp, 'NOT-REACHED');

    // A worker retires after three requests in a row end in a bailout. A
    // request that completes in between keeps this one in service.
    $t->assertMatch("$how: a request after it completes", $get('case=none'), '#^HTTP/\S+ 200 #');

    $slug = str_replace('_', '-', $how);
    $calls = Registry::counter("closure-fatal-pool-drop-$slug-calls-$run", static fn (): Counter => new Counter());
    $t->assertSame("$how: \$destroy ran once, and not after the fatal", $calls->get(), 1);
    $unfinishedAfter = $unfinished();
    $t->assertSame(
        "$how: the pool's teardown ran to its end",
        $unfinishedBefore === null || $unfinishedAfter === null ? null : $unfinishedAfter - $unfinishedBefore,
        0
    );
    $t->assertSame("$how: the slot it did not reach went without \$destroy", $skipped() - $skippedBefore, 1);
}

// The pool of the weak case was weakly referenced and a WeakMap key: both let
// go of it before its $destroy ran, and the WeakMap's value was released then.
$counter = static fn (string $name): int => Registry::counter($name, static fn (): Counter => new Counter())->get();
$t->assertSame(
    'weak: $destroy did not get the pool back through a weak reference',
    $counter("closure-fatal-pool-drop-weak-seen-$run"),
    0
);
$t->assertSame(
    'weak: the WeakMap released its value before the fatal',
    $counter("closure-fatal-pool-drop-weak-released-$run"),
    1
);

// Last: on a build that leaves the map's lock taken, this waits on it for good,
// and so does the one worker this profile has. The same key, so the same part
// of the map.
$t->assertContains(
    'map_clear: the map is usable after the fatal',
    $get("case=pool_drop_map_use&run=$run"),
    'MAP: v'
);

$t->done();
