<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/backtrace_probe.php';

// From PHP 8.5 a fatal takes a backtrace of where it was raised, with the
// arguments of every frame, and error_get_last() reports it under 'trace'. The
// engine keeps it on the thread, next to the last error, until the next error
// on that thread lets go of it — and a worker runs every request it serves on
// one thread. So it belongs to the request that fataled the way that request's
// last error does, and the same three things have to hold for it:
//
//   - a request that parks keeps its own, and finds it again on resume — here
//     from a shutdown function, which is where an application reads it;
//   - a request served in the window does not leave its own where another
//     request's error_get_last() reports it;
//   - and once that request has ended, its backtrace no longer holds what its
//     frames were given.
//
// Before 8.5 there is no backtrace, and every answer below is 'none'.

// Cleared so the warning below is recorded as one rather than turned into an
// exception by whichever handler the last request on this worker installed.
set_error_handler(null);
set_exception_handler(null);

$run = bin2hex(random_bytes(4));
$parkedSecret = "parked-$run";
$windowSecret = "window-$run";

/** Sends one request to this server and returns its socket, or false. */
$send = static function (string $query) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return false;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_backtrace_fatal.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    return $sock;
};

/** Reads a response to its end. Hooked: parks this request until it is all there. */
$read = static function ($sock): string {
    if ($sock === false) {
        return '';
    }
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

// Silenced, not unreported: @ only stops the display. error_get_last() records
// it either way, and an error of any kind lets go of whatever backtrace stood
// on the thread before this request — so this request starts with none.
@trigger_error('outer request warning', E_USER_WARNING);

$parked = $send("secret=$parkedSecret&park=1");

// Hooked: parks this request so the worker serves the first inner request, which
// fatals and then parks in its shutdown function.
usleep(300_000);

// The second one fatals while the first is parked, and runs to its end before
// this request reads on.
$window = $send("secret=$windowSecret");
$windowResp = $read($window);

// Read before anything else this request does can raise an error: any error
// lets go of the backtrace standing on the thread, which would hide the one this
// is looking for.
$mine = error_get_last();
$mineSays = OxphpBacktraceProbe::traceSays($mine);
$mineMessage = $mine['message'] ?? null;
unset($mine);
$held = OxphpBacktraceProbe::$held;
$heldGone = $held !== null && $held->get() === null;

$parkedResp = $read($parked);

$t = new TestCase('fatal_backtrace_stays_with_its_request', 'fibers');

$t->assertTrue('the first inner request connected', $parked !== false);
$t->assertTrue('the second inner request connected', $window !== false);
$t->assertContains('the second inner request reached its fatal', $windowResp, "ARMED:$windowSecret");
$t->assertNotContains('and nothing past it ran', $windowResp, 'NOT-REACHED');
$t->assertContains('the first inner request reached its fatal', $parkedResp, "ARMED:$parkedSecret");

// What the rest stands on: the second request fataled while the first was
// parked, read by the second at the moment it ran rather than from a flag the
// first only ever raises.
$t->assertContains('the second inner request ran while the first was parked', $windowResp, 'PEER-PARKED:1');

if (PHP_VERSION_ID >= 80500) {
    // Without these, none of the requests above takes a backtrace with
    // arguments, and every assertion below passes for nothing.
    $t->assertSame('fatal_error_backtraces is on', ini_get('fatal_error_backtraces'), '1');
    $t->assertFalse('zend.exception_ignore_args is off', (bool) ini_get('zend.exception_ignore_args'));
}

$expectOwn = PHP_VERSION_ID >= 80500 ? $parkedSecret : 'none';

$t->assertSame(
    'a shutdown function reads the backtrace of its own request\'s fatal',
    (string) preg_replace('/^.*TRACE-BEFORE-PARK:(\S+).*$/s', '$1', $parkedResp),
    $expectOwn
);
// The directive went with the request and was applied again on its way back
// in, which warns — so the resume raised an error before the read below.
$t->assertContains(
    'the directive that warns as it is applied went with the request and came back',
    $parkedResp,
    'SOCKET-TIMEOUT-AFTER-PARK:60x'
);
$t->assertSame(
    'and reads its own again after parking — not the one raised in the window, and not none',
    (string) preg_replace('/^.*TRACE-AFTER-PARK:(\S+).*$/s', '$1', $parkedResp),
    $expectOwn
);

$t->assertSame('this request still reads its own last error', $mineMessage, 'outer request warning');
$t->assertSame(
    'with no backtrace — it never fataled, and the fatal in the window was not its own',
    $mineSays,
    'none'
);

$t->assertTrue('the second inner request left a reference to what its fatal was given', $held !== null);
$t->assertTrue('and what its fatal was given is freed once that request has ended', $heldGone);

$t->done();
