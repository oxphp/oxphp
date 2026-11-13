<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/argument_probe.php';

// A fatal leaves a call that was being passed its arguments pending, and the
// worker releases the arguments passed so far. The engine warns about some
// arguments before it stores them — an undefined variable, a value unpacked by
// name into a by-reference parameter — and counts the one unpacked by name
// before the warning, so a fatal raised from that warning leaves the slot
// holding whatever the stack held there: in each inner request, the object the
// previous call at the same place was passed and has already given back.
// Released as an argument, it loses a reference it does not own, once per such
// fatal, and is freed while others still hold it. The argument stored ahead of
// it is still released: left alone, it keeps a reference for the life of the
// worker.
//
// Compiled by the tracing JIT, a call nested in the arguments of another may
// not be linked to the frame yet when the warning fatals. Then neither call's
// arguments are released: counted for the inner call and taken from the outer
// one's slots, they would include the object the outer call was passed on the
// previous turn. A fatal from the opcode that begins a call — an autoloader
// that fatals for `new` — still has the call outside it released.

OxphpArgumentProbe::$held ??= new stdClass();
OxphpArgumentProbe::$kept ??= new stdClass();

// Null when the inner request could not be sent.
$get = static function (string $path): ?string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    // Hooked: parks this request while the worker serves the inner one.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    return $resp;
};

$t = new TestCase('fatal_in_an_argument_warning_leaves_its_slot_alone', 'fibers');

foreach (['undefined', 'array', 'unpack', 'traced', 'autoload'] as $send) {
    $before = OxphpArgumentProbe::refcount();
    $keptBefore = OxphpArgumentProbe::keptRefcount();
    $resp = $get("fixture_argument_warning_fatal.php?send={$send}");
    $after = OxphpArgumentProbe::refcount();
    $keptAfter = OxphpArgumentProbe::keptRefcount();
    // A request that ends normally, so that three fatals in a row do not have
    // the worker — and this request with it — replaced.
    $get('fixture_fatal.php');

    $t->assertTrue("{$send}: the inner request connected", $resp !== null);
    $t->assertContains("{$send}: it fataled in the warning handler", (string) $resp, 'OxphpArgumentWarningFatal');
    $t->assertNotContains("{$send}: and nothing past the call ran", (string) $resp, 'NOT-REACHED');
    $t->assertTrue("{$send}: the object was counted before", $before > 0);
    $t->assertSame("{$send}: the object passed before keeps every reference it had", $after, $before);
    $t->assertTrue("{$send}: the argument stored ahead was counted before", $keptBefore > 0);
    $t->assertSame("{$send}: and the call gave back the reference it took", $keptAfter, $keptBefore);
}

$t->done();
