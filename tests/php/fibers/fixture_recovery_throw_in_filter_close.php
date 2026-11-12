<?php

declare(strict_types=1);

// Inner request for fibers/test_recovery_keeps_another_requests_exception_handler.
//
// Fatals while a frame holds a stream with a userland filter on it. Giving that
// handle back as the worker recovers from the fatal calls the filter's
// onClose(), and onClose() throws. The call has no frame of the request's
// beneath it to unwind into, so the engine hands the throw to whatever
// exception handler is installed on the thread — and nothing here installs one.

require_once __DIR__ . '/recovery_probe.php';
require_once __DIR__ . '/../engine_fatal.php';

if (!class_exists('OxphpRecoveryThrowOnCloseFilter', false)) {
    final class OxphpRecoveryThrowOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            OxphpRecoveryProbe::$closeRan = true;

            throw new \RuntimeException('thrown from a filter the worker was releasing');
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

if (!in_array('oxphp-recovery-throw-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-recovery-throw-on-close', OxphpRecoveryThrowOnCloseFilter::class);
}

if (!function_exists('oxphp_recovery_fatal_under_a_throwing_filter')) {
    function oxphp_recovery_fatal_under_a_throwing_filter(): void
    {
        $handle = fopen('php://memory', 'r+');
        stream_filter_append($handle, 'oxphp-recovery-throw-on-close');

        engine_fatal('OxphpRecoveryFatalUnderAThrowingFilter');
    }
}

oxphp_recovery_fatal_under_a_throwing_filter();
