<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A park inside a call of an error handler lends nothing while code PHP called
// on its own runs where a call the handler set off may still be running. A call
// of its own that has returned is not that: the other request's handler here
// installs a handler of its own, has it hear an error raised in its own frame,
// takes it off again and then parks in a plain call — and lends the worker the
// handler it is a call of, the one that stands once it returns.

$run = bin2hex(random_bytes(4));
$in = static fn (): bool => OxphpErrorModeProbe::$inWindow["inner-returned-$run"] ?? false;

$parked = OxphpErrorModeProbe::send("mode=inner-returned&run=$run", 'fixture_error_handler_park.php');
for ($i = 0; $i < 100 && !$in(); $i++) {
    usleep(20_000);
}
$before = $in();
// Read without changing it: set and put straight back.
$standing = set_error_handler(null);
restore_error_handler();
$inCall = $before && $in();
$parkedResp = OxphpErrorModeProbe::read($parked);

$t = new TestCase('error_handler_is_lent_from_a_call_once_a_handler_it_installed_has_returned', 'fibers');

$t->assertTrue('premise: the other request was parked inside its handler while this one looked', $inCall);
$t->assertSame('its handler stands for this one', get_debug_type($standing), 'Closure');
$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");

$t->done();
