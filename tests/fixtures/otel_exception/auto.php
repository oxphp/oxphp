<?php
// Automatic path: a #[OxPHP\Apm\Trace] function that throws. The decorator
// records an "exception" span event with type/message/stacktrace.
use OxPHP\Apm\Trace;

#[Trace]
function chargeCard(): void {
    throw new RuntimeException('auto path: card declined');
}

try {
    chargeCard();
} catch (\Throwable $e) {
    // swallow — the decorator already recorded the event on span exit
}
echo "auto ok\n";
