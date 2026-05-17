<?php
/**
 * Regression: Channel::recv() inside a user-level Fiber must not raise.
 *
 * Before the SAPI-callback fix for `oxphp_bridge_in_fiber()`, the bridge
 * predicate flipped to true inside a `Fiber::start()` body (the user
 * Fiber installs its own zend_fiber_context, distinct from
 * main_fiber_context). The Channel handler then took the fiber-suspend
 * path, called `oxphp_bridge_fiber_await`, and got rc=1 ("not in oxphp
 * fiber") because the SAPI's `oxphp_current_fiber` __thread pointer is
 * only set by `oxphp_scheduler_resume_fiber`. The unmatched arm raised
 * `RuntimeException: recv: fiber_await rc=1`. After the fix, the
 * predicate keys off `oxphp_current_fiber` directly via a registered
 * callback, so user fibers correctly take the thread-blocking branch.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);
$ch->send('x');

$f = new Fiber(function () use ($ch) {
    echo $ch->recv()->value() . "\n";
});
$f->start();

echo "OK\n";
