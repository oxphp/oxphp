<?php
// Non-UTF-8 message path: 0xE9 is 'é' in latin1 — a lone byte that is invalid
// UTF-8. Before length-delimited + lossy capture, the whole message was dropped.
use OxPHP\Apm\Trace;

#[Trace]
function boomLatin1(): void {
    throw new RuntimeException("caf\xE9 latin1 error");
}

try {
    boomLatin1();
} catch (\Throwable $e) {
    // swallow
}
echo "latin1 ok\n";
