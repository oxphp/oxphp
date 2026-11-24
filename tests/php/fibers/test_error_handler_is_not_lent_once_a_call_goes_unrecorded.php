<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// The server keeps a record of a limited number of calls of an error handler per
// request, and the copies of handlers the request removed take up their room
// until it ends. Once a call goes unrecorded, no park of the request lends the
// worker a handler: a handler that is not a Closure is told from another only by
// where its call's frame stands, and the unrecorded call, made from the same
// statement as a recorded one, stands where that one did. Lent, the handler of
// that record would be one the request has removed. The other request here sets
// nine handler objects in turn, each heard once from the same statement and
// removed, and parks inside the ninth call.

$run = bin2hex(random_bytes(4));
$in = static fn (): bool => OxphpErrorModeProbe::$inWindow["saturated-$run"] ?? false;

$parked = OxphpErrorModeProbe::send("mode=saturate&run=$run", 'fixture_error_handler_park.php');
for ($i = 0; $i < 100 && !$in(); $i++) {
    usleep(20_000);
}
$before = $in();
// Read without changing it: set and put straight back.
$standing = set_error_handler(null);
restore_error_handler();
$inCall = $before && $in();
$parkedResp = OxphpErrorModeProbe::read($parked);

$t = new TestCase('error_handler_is_not_lent_once_a_call_goes_unrecorded', 'fibers');

$t->assertTrue('premise: the other request was parked inside its ninth call while this one looked', $inCall);
$t->assertSame(
    'no handler stands for this one',
    $standing instanceof OxphpErrorHandlerCollector ? "handler-{$standing->id}" : get_debug_type($standing),
    'null'
);
$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");

$t->done();
