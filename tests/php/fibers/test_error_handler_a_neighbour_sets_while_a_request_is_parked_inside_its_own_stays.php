<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A request parked inside its error handler has lent the worker that handler. A
// request served meanwhile that sets a handler of its own replaces it for the
// worker, as it would replace any handler: once the parked one's handler
// returns, the worker's handler is the one set last, not the one that was
// running.

$run = bin2hex(random_bytes(4));

$parked = OxphpErrorModeProbe::send("mode=single&run=$run", 'fixture_error_handler_park.php');
for ($i = 0; $i < 100 && !(OxphpErrorModeProbe::$inWindow["in-handler-$run"] ?? false); $i++) {
    usleep(20_000);
}
$inHandler = OxphpErrorModeProbe::$inWindow["in-handler-$run"] ?? false;
set_error_handler(static function (int $severity, string $message): bool {
    throw new \ErrorException("neighbour-handler:$message", 0, $severity);
});
$stillInHandler = OxphpErrorModeProbe::$inWindow["in-handler-$run"] ?? false;
$parkedResp = OxphpErrorModeProbe::read($parked);

try {
    trigger_error("probe-$run", E_USER_WARNING);
    $thrown = 'none';
} catch (\Throwable $e) {
    $thrown = get_class($e) . ':' . $e->getMessage();
}
// Back past this request's handler, the parked one's, and what stood before it.
restore_error_handler();
restore_error_handler();

$t = new TestCase('error_handler_a_neighbour_sets_while_a_request_is_parked_inside_its_own_stays', 'fibers');

$t->assertTrue('premise: the other request was parked inside its handler while this one set its own', $inHandler && $stillInHandler);
$t->assertContains('once its handler returned, the worker\'s handler was no longer its own', $parkedResp, "HANDLER-AFTER:other\n");
$t->assertSame('the worker\'s handler is the one this request set', $thrown, "ErrorException:neighbour-handler:probe-$run");
$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");

$t->done();
