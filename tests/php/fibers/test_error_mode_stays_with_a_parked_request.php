<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// `new PDO()` and `new SplFileObject()` run their open in a mode that turns a
// warning into an exception and skips the application's error handler, and put
// the previous mode back when the open returns. The mode is the thread's, and a
// worker runs every request it serves on one thread. A request that parks inside
// such an open — a database connect, here a stream wrapper's sleep — has to take
// its mode with it:
//
//   - a request that runs meanwhile raises its errors in its own mode, so its
//     own handler hears them;
//   - the parked request finds its own mode again when it resumes;
//   - and two such opens that end in the order they began, not the reverse,
//     leave the worker in the mode it was in, for this request and the next.

$run = bin2hex(random_bytes(4));
$first = "first-$run";
$second = "second-$run";

// Hooked sleeps: each parks this request so the worker serves the request just
// sent, which enters its open and parks there.
$one = OxphpErrorModeProbe::send('open=' . rawurlencode("oxehpark://park/900/$first"));
usleep(200_000);
$two = OxphpErrorModeProbe::send('open=' . rawurlencode("oxehpark://park/900/$second"));
usleep(200_000);

$bothIn = (OxphpErrorModeProbe::$inWindow[$first] ?? false) && (OxphpErrorModeProbe::$inWindow[$second] ?? false);
$during = OxphpErrorModeProbe::measure('during');
$bothStillIn = (OxphpErrorModeProbe::$inWindow[$first] ?? false) && (OxphpErrorModeProbe::$inWindow[$second] ?? false);

// The first one began first and ends first.
$oneResp = OxphpErrorModeProbe::read($one);
$twoResp = OxphpErrorModeProbe::read($two);

$after = OxphpErrorModeProbe::measure('after');
$fresh = OxphpErrorModeProbe::read(OxphpErrorModeProbe::send('measure=fresh'));

$t = new TestCase('error_mode_stays_with_a_parked_request', 'fibers');

$t->assertTrue('both opens were parked inside their window while this request measured', $bothIn && $bothStillIn);

$t->assertSame('a request that runs meanwhile: its own handler hears its errors', $during['heard'], ['notice-during', 'warning-during']);
$t->assertSame('and nothing is thrown in their place', $during['thrown'], 'none');
$t->assertSame('or printed', $during['shown'], '');

$t->assertContains('the first open resumes in its own mode: its warning is what it throws', $oneResp, "OPEN:RuntimeException:after-park-$first\n");
$t->assertContains('and so does the second, which ended after it', $twoResp, "OPEN:RuntimeException:after-park-$second\n");

$t->assertSame('this request, once both opens have ended, is back in its own mode', $after['heard'], ['notice-after', 'warning-after']);
$t->assertSame('and nothing is thrown', $after['thrown'], 'none');
$t->assertContains(
    'a request that starts after both opens ended starts in the normal mode',
    $fresh,
    'MEASURE:{"heard":["notice-fresh","warning-fresh"],"thrown":"none","shown":""}'
);

$t->done();
