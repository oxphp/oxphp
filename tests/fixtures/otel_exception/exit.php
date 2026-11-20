<?php
// exit() inside a #[OxPHP\Apm\Trace] function: the frame is unwound with the
// engine's UnwindExit, which is not a Throwable. The call finished, so its span is
// exported without an error status or an exception event, and the request ends as
// exit() ends any request: its shutdown function still writes to the response.
use OxPHP\Apm\Trace;

#[Trace]
function leaves(): void {
    echo "exit body\n";
    exit();
}

register_shutdown_function(static function (): void {
    echo "exit shutdown\n";
});

leaves();
