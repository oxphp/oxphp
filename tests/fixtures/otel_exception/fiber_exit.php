<?php
// A Fiber destroyed while it is suspended inside a #[OxPHP\Apm\Trace] function:
// the engine unwinds the frame with its GracefulExit, which is not a Throwable.
// The call finished, so its span is exported without an error status or an
// exception event, and the script goes on past the destruction.
use OxPHP\Apm\Trace;

#[Trace]
function parks(): void {
    Fiber::suspend();
}

$fiber = new Fiber(static function (): void {
    parks();
});
$fiber->start();
unset($fiber);
echo "fiber exit went on\n";
