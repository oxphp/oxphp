<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// A fatal raised inside an open that runs in the mode turning warnings into
// exceptions — `new DirectoryIterator()` here, `new PDO()` alike — never reaches the
// line that puts the previous mode back. A PHP that ends the request after a
// fatal starts the next one in the normal mode; a worker goes on serving on the
// same thread, so it has to do that itself, wherever a request or task starts:
//
//   - a request the worker takes with nothing else in flight — this test, run by
//     the suite right after the line whose request fataled that way;
//   - a request the worker takes while another is parked;
//   - a task the async thread takes after a task fataled that way.

// Measured first, before anything else this request does. The premise is taken
// down once read, so a run of this test without that line does not stand on a
// fatal an earlier run left.
$own = OxphpErrorModeProbe::measure('own');
$suiteFataled = OxphpErrorModeProbe::$fataling['suite'] ?? false;
unset(OxphpErrorModeProbe::$fataling['suite']);

$tag = bin2hex(random_bytes(4));

// The request after the fatal is sent by the fataling one, just before its open:
// the worker takes it once that one's slice has ended, while this request is still
// asleep, so nothing resumed in between puts its own mode back on the thread.
$fatal = OxphpErrorModeProbe::send('open=' . rawurlencode("oxehpark://fatal/$tag") . '&send=' . rawurlencode("measure=next&after=$tag"));
usleep(500_000);
$fatalResp = OxphpErrorModeProbe::read($fatal);
$nextResp = OxphpErrorModeProbe::read(OxphpErrorModeProbe::$sent[$tag] ?? false);
unset(OxphpErrorModeProbe::$sent[$tag]);

// Kept parked across the two tasks below: with nothing in flight the pool may
// reset the thread between tasks, which would hide what the fatal task left.
$sleeper = oxphp_async(static function (): void {
    oxphp_sleep(1.0);
});
$fatalTask = oxphp_async(static function () use ($tag): void {
    // The task runs on another thread, which has classes of its own.
    require_once __DIR__ . '/error_mode_probe.php';
    OxphpErrorModeProbe::open("oxehpark://fatal/$tag");
});
$taskFataled = false;
try {
    oxphp_async_await($fatalTask, 5.0);
} catch (\Throwable $e) {
    $taskFataled = str_contains($e->getMessage(), 'OxphpErrorModeFatal');
}
$nextTask = oxphp_async_await(oxphp_async(static function (): array {
    require_once __DIR__ . '/error_mode_probe.php';

    return OxphpErrorModeProbe::measure('next-task');
}), 5.0);
oxphp_async_await($sleeper, 5.0);

$t = new TestCase('fatal_inside_an_error_mode_window_spares_what_runs_next', 'fibers');

$t->assertTrue('the suite line before this one fataled inside its open', $suiteFataled);
$t->assertSame('a request taken after the fatal with nothing in flight: its own handler hears its errors', $own['heard'], ['notice-own', 'warning-own']);
$t->assertSame('and nothing is thrown in their place', $own['thrown'], 'none');

$t->assertContains('the request this one sent reached its open', $fatalResp, "OPENING:oxehpark://fatal/$tag");
$t->assertNotContains('and the open never returned', $fatalResp, 'OPEN:');
$t->assertContains('the request it sent started once it had fataled', $nextResp, 'PEER-FATALED:1');
$t->assertContains(
    'and, taken while this one was parked, starts in the normal mode',
    $nextResp,
    'MEASURE:{"heard":["notice-next","warning-next"],"thrown":"none","shown":""}'
);

$t->assertTrue('the first task fataled inside its open', $taskFataled);
$t->assertSame('the task taken after it: its own handler hears its errors', $nextTask['heard'], ['notice-next-task', 'warning-next-task']);
$t->assertSame('and nothing is thrown in their place', $nextTask['thrown'], 'none');

$t->done();
