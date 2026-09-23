<?php

declare(strict_types=1);

// A destructor that runs as a request's ini is put back, and changes more ini on
// its way, must not corrupt the set being put back.
//
// The same shape as test_ini_put_back_does_not_park: putting assert.callback back
// drops a closure, and with it the object below. Its destructor sets directives
// the request had not touched, which adds each of them to the thread's set of
// modified directives — enough of them that the set has to grow, while the
// put-back is in the middle of going over it. A put-back that walked the live set
// would be holding a pointer into the storage the growth just freed, and would
// remove assert.callback's entry through it: read its hash and key from there,
// and write there. Freed storage that nothing has taken yet still holds the old
// entry, which hides that, so the destructor then takes it over with strings of
// its own. The probe on the next line reads that the worker came through it,
// that those strings are intact, and that none of what the destructor set is
// standing in the next request.
//
// Some of what it sets does not travel — default_charset, the pcre limits,
// url_rewriter.tags — and is put back only by the last request out, which this
// one is. Put back, that is, only if the put-back reaches entries added while it
// runs. A worker that goes idle next rolls every directive back anyway and would
// hide one it missed, so this request leaves a task behind for the probe, as
// test_ini_is_the_requests_own does: a worker still running a task is not idle,
// and the probe is taken without that reset.

require_once __DIR__ . '/ini_put_back_probe.php';

set_error_handler(null);
set_exception_handler(null);

if (!class_exists('OxphpIniPutBackGrower', false)) {
    final class OxphpIniPutBackGrower
    {
        public function __destruct()
        {
            OxphpIniPutBackProbe::$growerRan = true;
            foreach (OxphpIniPutBackProbe::GROWN as $name => $value) {
                ini_set($name, $value);
            }
            // Then takes the storage the growth gave up. The allocator hands a
            // size class's most recently freed block out first, so one string
            // per size class the set's storage can have had lands on it. A
            // put-back still holding a pointer into that storage then reads
            // this text as the entry it was removing, and writes into it.
            OxphpIniPutBackProbe::$fill = [];
            foreach (OxphpIniPutBackProbe::FILL_LENGTHS as $i => $length) {
                OxphpIniPutBackProbe::$fill[] = str_repeat(chr(0x61 + $i), $length);
            }
        }
    }
}

OxphpIniPutBackProbe::$growerRan = false;

// Both calls are deprecated; the deprecations are not what this is about.
@ini_set('assert.callback', 'strlen');
$grower = new OxphpIniPutBackGrower();
@assert_options(ASSERT_CALLBACK, static function () use ($grower): void {
});
unset($grower);

// is_file before unlink, not the silence operator: a registered error handler
// still runs under @.
if (is_file(OxphpIniPutBackProbe::GROWER_TASK_DONE)) {
    unlink(OxphpIniPutBackProbe::GROWER_TASK_DONE);
}
oxphp_async(static function (): void {
    time_nanosleep(2, 0);
    file_put_contents(OxphpIniPutBackProbe::GROWER_TASK_DONE, 'done');
});

echo 'set';
