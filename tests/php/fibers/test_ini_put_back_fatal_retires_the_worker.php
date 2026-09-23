<?php

declare(strict_types=1);

// A destructor that runs as a request's ini is put back, and has a fatal, retires
// the worker.
//
// The same shape as test_ini_put_back_reports_a_throw, with a fatal in place of
// the throw. The fatal leaves the handler that ran the destructor part-way
// through: assert.callback's frees the callback it holds and only then forgets
// it, so the callback is left pointing at a closure that is half freed, for the
// next failed assert() on this worker to call. Nothing short of a fresh worker
// puts that right. The suite line expects the 500 a fatal answers; the probe on
// the next line reads that the destructor ran and that the worker was recycled
// on its own schedule — which nothing else in this profile asks for.

require_once __DIR__ . '/ini_put_back_probe.php';

set_error_handler(null);
set_exception_handler(null);

if (!class_exists('OxphpIniPutBackFatal', false)) {
    final class OxphpIniPutBackFatal
    {
        public function __destruct()
        {
            $state = json_decode((string) file_get_contents(OxphpIniPutBackProbe::FATAL_STATE), true);
            $state['ran'] = true;
            file_put_contents(OxphpIniPutBackProbe::FATAL_STATE, json_encode($state));
            // Deprecated as of PHP 8.4, and still a fatal.
            trigger_error('a fatal in a destructor the ini put-back ran', E_USER_ERROR);
        }
    }
}

file_put_contents(OxphpIniPutBackProbe::FATAL_STATE, json_encode([
    'ran' => false,
    'scheduled' => oxphp_ini_put_back_scheduled_recycles(),
]));

// Both calls are deprecated; the deprecations are not what this is about.
@ini_set('assert.callback', 'strlen');
$fatal = new OxphpIniPutBackFatal();
@assert_options(ASSERT_CALLBACK, static function () use ($fatal): void {
});
unset($fatal);

echo 'set';
