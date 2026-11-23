<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// In worker mode the error handler is the worker's: set_error_handler() in one
// request installs it for every request the worker serves after. While PHP runs a
// handler it takes it off the thread, so an error the handler itself raises is
// not handed back to it, and puts it back when the handler returns. A request
// that parks inside its handler — a hooked sleep or read there — leaves that
// window open while the worker serves others, and they have to run with the
// handler the worker has:
//
//   - parked inside the handler: a request served meanwhile has its errors heard
//     by that handler; the parked one, resumed, is still inside it, and an error
//     it raises there is not handed back to it;
//   - parked inside the handler past a set_error_handler(null) of the handler's
//     own: none is what the worker has;
//   - parked once the handler has returned, with none installed at the depth the
//     handler ran at: none again.

$run = bin2hex(random_bytes(4));

// No set_error_handler() here: what is measured is the worker's handler.
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
$in = static fn (string $what): bool => OxphpErrorModeProbe::$inWindow["$what-$run"] ?? false;
$measure = static function (string $what) use ($in, $probe, $run): array {
    for ($i = 0; $i < 100 && !$in($what); $i++) {
        usleep(20_000);
    }
    $before = $in($what);
    $seen = $probe("$what-$run");

    return [$before && $in($what), $seen];
};

$parked = OxphpErrorModeProbe::send("mode=phases&run=$run", 'fixture_error_handler_park.php');
[$inHandler, $during] = $measure('in-handler');
[$inNulled, $nulled] = $measure('nulled');
[$inBare, $bare] = $measure('bare');
$parkedResp = OxphpErrorModeProbe::read($parked);

$t = new TestCase('error_handler_stays_with_the_worker_while_a_request_is_parked_inside_it', 'fibers');

$t->assertTrue('premise: the other request was parked inside its handler while this one raised', $inHandler);
$t->assertSame(
    'a request served meanwhile: the handler hears its warning',
    $during['thrown'],
    "ErrorException:parked-handler:probe-in-handler-$run"
);
$t->assertSame('and nothing is printed in its place', $during['shown'], '');
$t->assertContains('the parked one, resumed inside its handler: an error raised there is not handed back to it', $parkedResp, "REENTERED:0\n");

$t->assertTrue('premise: parked inside its handler again, past a set_error_handler(null) of its own', $inNulled);
$t->assertSame('a request served then runs with no handler: nothing thrown', $nulled['thrown'], 'none');
$t->assertContains('and the warning printed', $nulled['shown'], "probe-nulled-$run");

$t->assertContains('the handler is the worker\'s again once it returns', $parkedResp, "HANDLER-AFTER:own\n");

$t->assertTrue('premise: parked once its handler had returned, with none installed at the depth it ran at', $inBare);
$t->assertSame('a request served then runs with no handler: nothing thrown', $bare['thrown'], 'none');
$t->assertContains('and the warning printed', $bare['shown'], "probe-bare-$run");
$t->assertContains('the other request ran to its end', $parkedResp, "DONE\n");

$t->done();
