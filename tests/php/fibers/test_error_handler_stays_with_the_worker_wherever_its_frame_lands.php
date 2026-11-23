<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A request parked inside its error handler leaves the worker that handler for
// the requests served meanwhile wherever the handler's frame landed on the VM
// stack — past the end of a stack page too, where PHP starts the frame on a new
// page. The other request raises its error from frames four arguments wider each
// time, so the handler's frame steps across a page four slots at a time, and each
// time parks inside the handler until this request has raised a warning.

$run = bin2hex(random_bytes(4));
$flag = static fn (string $what): bool => OxphpErrorModeProbe::$inWindow["$what-$run"] ?? false;

$parked = OxphpErrorModeProbe::send("mode=pages&run=$run", 'fixture_error_handler_park.php');
$probes = 0;
$unheard = [];
for ($i = 0; $i < 20_000 && !$flag('pages-done'); $i++) {
    if (!$flag('page') || $flag('probed')) {
        usleep(1_000);
        continue;
    }
    ob_start();
    try {
        trigger_error("probe-$run", E_USER_WARNING);
        $unheard[] = $probes;
    } catch (\ErrorException $e) {
        if ($e->getMessage() !== "parked-handler:probe-$run") {
            $unheard[] = $probes;
        }
    }
    ob_end_clean();
    $probes++;
    OxphpErrorModeProbe::$inWindow["probed-$run"] = true;
}
$parkedResp = OxphpErrorModeProbe::read($parked);

$t = new TestCase('error_handler_stays_with_the_worker_wherever_its_frame_lands', 'fibers');

$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");
$t->assertContains(
    'premise: this request raised a warning at each of the other one\'s parks inside its handler',
    $parkedResp,
    "PARKS:$probes\n"
);
// Listed by park, the first one 0: the other request's frame was 4 arguments
// wider at each park than at the one before.
$t->assertSame('the handler heard every one of them', $unheard, []);

$t->done();
