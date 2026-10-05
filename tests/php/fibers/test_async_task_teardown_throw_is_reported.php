<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// An oxphp_async() task ends by letting go of what it leaves on its thread: the
// shutdown functions it registered, and from PHP 8.5 the backtrace of a fatal it
// raised, which holds what that fatal's frames were given. Either can drop the
// last reference to an object whose destructor throws, after the task's own
// result has been taken. Nothing is left to catch that exception, so unless the
// end of the task reports it, nothing does — where a request reports the same
// exception, thrown in its shutdown window, as an uncaught one.
//
// Each task below leaves such an object behind and returns normally. The log the
// report goes to is a file the task points error_log at; the profile runs one
// async worker, so a last task puts those directives back for the tests after
// this one.

$t = new TestCase('async_task_teardown_throw_is_reported', 'fibers');

$base = sys_get_temp_dir() . '/oxphp-task-teardown-' . bin2hex(random_bytes(6));
$log = $base . '.log';

/** What the log holds once it holds $needle, or by the deadline. */
$logAfter = static function (string $needle) use ($log): string {
    $deadline = microtime(true) + 3.0;
    do {
        $logged = is_file($log) ? (string) file_get_contents($log) : '';
        if (str_contains($logged, $needle)) {
            break;
        }
        usleep(50_000);
    } while (microtime(true) < $deadline);

    return $logged;
};

// A shutdown function the task registers holds the object; the free of the
// task's registrations at its end drops it.
$registryMarker = $base . '.registry';
$registry = oxphp_async(static function (string $log, string $marker): string {
    ini_set('display_errors', '0');
    ini_set('log_errors', '1');
    ini_set('error_log', $log);
    require_once __DIR__ . '/task_teardown_thrower.php';

    $thrower = new OxphpTaskTeardownThrower($marker, 'registry');
    register_shutdown_function(static function () use ($thrower): void {
    });
    unset($thrower);

    return 'returned';
}, $log, $registryMarker);

$t->assertSame('the task\'s outcome is what its closure returned', oxphp_async_await($registry, 5.0), 'returned');
$logged = $logAfter('task teardown throw: registry');
$t->assertTrue('the destructor ran once the task had ended', is_file($registryMarker));
$t->assertContains(
    'and the exception it threw at the free of the task\'s shutdown functions was reported',
    $logged,
    'Uncaught RuntimeException: task teardown throw: registry'
);

if (PHP_VERSION_ID >= 80500) {
    // The backtrace of a fatal a handler took over holds the object: such a
    // fatal neither ends the task nor flags anything as destructed. Its outcome
    // being what the closure returned is what says the object outlived the
    // closure — had nothing held it, its destructor would have thrown into the
    // closure as the call returned.
    $backtraceMarker = $base . '.backtrace';
    $backtrace = oxphp_async(static function (string $log, string $marker): string {
        ini_set('display_errors', '0');
        ini_set('log_errors', '1');
        ini_set('error_log', $log);
        require_once __DIR__ . '/task_teardown_thrower.php';

        set_error_handler(static fn (): bool => true);
        $fail = static function (object $held): void {
            trigger_error('task teardown probe fatal', E_USER_ERROR);
        };
        $fail(new OxphpTaskTeardownThrower($marker, 'backtrace'));
        restore_error_handler();

        return 'returned';
    }, $log, $backtraceMarker);

    $t->assertSame(
        'the fatal task\'s outcome is what its closure returned — its backtrace held the object past it',
        oxphp_async_await($backtrace, 5.0),
        'returned'
    );
    $logged = $logAfter('task teardown throw: backtrace');
    $t->assertTrue('the destructor ran once the task had ended', is_file($backtraceMarker));
    $t->assertContains(
        'and the exception it threw at the release of the fatal\'s backtrace was reported',
        $logged,
        'Uncaught RuntimeException: task teardown throw: backtrace'
    );
}

oxphp_async_await(oxphp_async(static function (): void {
    ini_restore('display_errors');
    ini_restore('log_errors');
    ini_restore('error_log');
}), 5.0);

foreach ([$log, $registryMarker, $backtraceMarker ?? null] as $file) {
    if ($file !== null && is_file($file)) {
        unlink($file);
    }
}

$t->done();
