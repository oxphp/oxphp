<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A park inside a call of an error handler lends the worker the handler that
// will stand once the call returns: the one it was made with, unless the call
// changes it. A call PHP makes through __call() is not told from code that runs
// in its place, so a park inside one lends nothing — not the handler of a call
// it is made inside of, either. The other request's handler here, inside its own
// call, puts back the handler under it and installs one PHP reaches through
// __call(), which then hears an error raised there and parks. Once both calls
// return, the one installed inside stands; lent meanwhile, the outer one would
// be a handler the request has removed.

$run = bin2hex(random_bytes(4));
$in = static fn (): bool => OxphpErrorModeProbe::$inWindow["via-call-$run"] ?? false;

$parked = OxphpErrorModeProbe::send("mode=via-call&run=$run", 'fixture_error_handler_park.php');
for ($i = 0; $i < 100 && !$in(); $i++) {
    usleep(20_000);
}
$before = $in();
// Read without changing it: set and put straight back.
$standing = set_error_handler(null);
restore_error_handler();
$inCall = $before && $in();
$parkedResp = OxphpErrorModeProbe::read($parked);

$t = new TestCase('error_handler_is_not_lent_from_outside_a_call_made_through_call', 'fibers');

$t->assertTrue('premise: the other request was parked inside the call through __call() while this one looked', $inCall);
$t->assertSame('no handler stands for this one', get_debug_type($standing), 'null');
$t->assertContains('the handler installed inside the outer call stands once both return', $parkedResp, "HANDLER-AFTER:via-call\n");
$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");

$t->done();
