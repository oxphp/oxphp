<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('select_unwatchable_fd', 'hooks');

// A descriptor the kernel's readiness interface will not watch at all. A regular
// file is the ordinary case: select() calls one permanently ready, which is why
// PHP accepts it, but a readiness registration for it is refused outright.
//
// The hook must therefore decline the wait and hand the call over, and this is
// the shape that proves it: a file that has no readiness event to deliver, given
// a timeout long enough to be obvious. Declining answers at once; parking on it
// would wait out the whole two seconds and then report a timeout on a descriptor
// PHP considers ready. With a null timeout — a shape real select loops use — the
// same mistake would never return at all.
//
// It reaches the registration by construction: the file is opened and not read,
// so nothing is buffered to short-circuit on, and a plain file casts to a
// descriptor, so the earlier decline paths do not fire. Unlike php://memory,
// which has no descriptor and is turned away before any of this.
$probe = static function (): array {
    $fh = fopen(__FILE__, 'r');

    $read = [$fh];
    $write = null;
    $except = null;

    $t0 = microtime(true);
    $ready = stream_select($read, $write, $except, 2);
    $elapsed = microtime(true) - $t0;

    $kept = count($read) === 1 && $read[array_key_first($read)] === $fh;
    fclose($fh);

    return ['ready' => $ready, 'elapsed' => $elapsed, 'kept' => $kept];
};

$native = $probe();
$hooked = oxphp_async_await(oxphp_async($probe), 10.0);

foreach (['native' => $native, 'hooked' => $hooked] as $label => $r) {
    $t->assertSame("{$label}: the file was reported ready", $r['ready'], 1);
    $t->assertTrue("{$label}: the file was kept in the read array", $r['kept']);
    $t->assertLessThan("{$label}: the call answered at once instead of waiting", $r['elapsed'], 0.5);
}

$t->done();
