<?php

declare(strict_types=1);

// A request that parks long enough to still be parked when the worker it runs
// on is torn down. Deliberately not a TestCase: it is the subject of the
// shutdown check, not a test of its own.
//
// The local holding the current fiber is the point of the fixture, not
// decoration: it keeps a second reference to the fiber object alive for as long
// as this frame is parked, so releasing the scheduler's reference at teardown
// cannot be what unwinds the request. Without it the check would pass on a
// teardown that only ever handles a fiber nobody else refers to.
//
// ?exit=1 makes the worker tear itself down while this request is parked. The
// exit is checked between scheduler ticks, so it lands with this fiber
// suspended mid-request rather than after it finishes.
header('Content-Type: text/plain');

if (isset($_GET['exit'])) {
    OxPHP\Server\Worker::current()->scheduleExit();
}

$self = \Fiber::getCurrent();
echo "parked\n";
oxphp_sleep(3.0);
echo "resumed by ", $self === \Fiber::getCurrent() ? 'itself' : 'another fiber', "\n";
