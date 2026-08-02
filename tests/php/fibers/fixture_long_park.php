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

// Ending a request means running these, and running them means the registry
// they sit in is still the one this request registered into — a request torn
// down without being ended never reaches them at all.
//
// Written to a file rather than echoed because this request is ended by a
// cancellation, and output from a cancelled request re-raises the cancellation
// at the first write; the file is the only marker that survives that.
register_shutdown_function(static function (): void {
    @file_put_contents('/tmp/oxphp-parked-shutdown-ran', "ran\n");
});

if (isset($_GET['exit'])) {
    OxPHP\Server\Worker::current()->scheduleExit();
}

$self = \Fiber::getCurrent();
echo "parked\n";
oxphp_sleep(3.0);
echo "resumed by ", $self === \Fiber::getCurrent() ? 'itself' : 'another fiber', "\n";
