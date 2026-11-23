<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A request parked inside a call of its error handler lends the worker that
// handler; one whose call has returned lends nothing, even parked in a callback
// whose frame takes the place the handler's had. With a handler that is not a
// Closure, what tells the two apart is that PHP calls a handler with four
// arguments: the request here parks in array_map()'s callback, which gets one.

$run = bin2hex(random_bytes(4));

$probe = static function (string $tag): array {
    ob_start();
    try {
        trigger_error("probe-$tag", E_USER_WARNING);
        $thrown = 'none';
    } catch (\Throwable $e) {
        $thrown = get_class($e) . ':' . $e->getMessage();
    }

    return ['thrown' => $thrown, 'shown' => (string) ob_get_clean()];
};
$in = static fn (): bool => OxphpErrorModeProbe::$inWindow["callback-$run"] ?? false;

$parked = OxphpErrorModeProbe::send("mode=callable&run=$run", 'fixture_error_handler_park.php');
for ($i = 0; $i < 100 && !$in(); $i++) {
    usleep(20_000);
}
$before = $in();
$seen = $probe("callback-$run");
$inCallback = $before && $in();
$parkedResp = OxphpErrorModeProbe::read($parked);

$t = new TestCase('error_handler_is_not_lent_from_a_callback_in_the_place_of_its_returned_call', 'fibers');

$t->assertContains('premise: the other request\'s handler was heard and returned', $parkedResp, "HEARD:1\n");
$t->assertTrue('premise: it was parked in the callback while this one raised', $inCallback);
$t->assertSame('a request served then runs with no handler: nothing thrown', $seen['thrown'], 'none');
$t->assertContains('and the warning printed', $seen['shown'], "probe-callback-$run");
$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");

$t->done();
