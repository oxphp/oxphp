<?php

declare(strict_types=1);

// Inner request for fibers/test_recovery_after_handler_holds_the_worker
// (?case=sleep) and fibers/test_recovery_after_handler_throw_keeps_the_worker
// (?case=throw).
//
// A decorated function fatals. The fatal leaves the decorated call open, and
// the worker closes it as it recovers from the fatal, which calls the
// decorator's after(): application code, run while the thread still carries
// the fatal. ?case=sleep sleeps there on a hooked usleep(), which is a point a
// request can park at. ?case=throw throws an exception whose class fatals in
// __destruct, so dropping that exception raises a second fatal inside the
// recovery.

require_once __DIR__ . '/recovery_probe.php';
require_once __DIR__ . '/../engine_fatal.php';

if (!class_exists('OxphpRecoveryFatalOnDestructException', false)) {
    final class OxphpRecoveryFatalOnDestructException extends \RuntimeException
    {
        public function __destruct()
        {
            OxphpRecoveryProbe::$stage = 'exception-destructing';
            engine_fatal('OxphpRecoveryExceptionDestructorFatal');
        }
    }
}

if (!class_exists('OxphpRecoveryAfterDecorator', false)) {
    #[\Attribute(\Attribute::TARGET_FUNCTION)]
    final class OxphpRecoveryAfterDecorator implements \OxPHP\Decorator\AttributeInterface
    {
        public function before(\OxPHP\Decorator\Context $ctx): void
        {
        }

        public function after(\OxPHP\Decorator\Context $ctx): void
        {
            if (OxphpRecoveryProbe::$case === 'sleep') {
                OxphpRecoveryProbe::$stage = 'after-sleeping';
                // Hooked. Long enough for the outer request's poll to come
                // round many times, if this lets it.
                usleep(300_000);
                OxphpRecoveryProbe::$stage = 'after-slept';
            } elseif (OxphpRecoveryProbe::$case === 'throw') {
                OxphpRecoveryProbe::$stage = 'after-throwing';
                throw new OxphpRecoveryFatalOnDestructException('thrown from after() while the worker recovers');
            }
        }
    }

    oxphp_register_decorator(OxphpRecoveryAfterDecorator::class);
}

if (!function_exists('oxphp_recovery_decorated_fatal')) {
    #[OxphpRecoveryAfterDecorator]
    function oxphp_recovery_decorated_fatal(): void
    {
        engine_fatal('OxphpRecoveryDecoratedFatal');
    }
}

OxphpRecoveryProbe::$case = (string) ($_GET['case'] ?? '');
OxphpRecoveryProbe::$stage = 'calling';
oxphp_recovery_decorated_fatal();
