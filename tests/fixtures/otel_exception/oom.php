<?php
// A #[OxPHP\Apm\Trace] function that runs past memory_limit. From PHP 8.4.27 /
// 8.5.12 the engine reports that while the function's frame is running and
// unwinds it with the exception exit() raises, which is not a Throwable. The call
// failed, so its span is exported with an error status, and without an exception
// event: there is no Throwable to describe.
use OxPHP\Apm\Trace;

ini_set('memory_limit', '16M');

#[Trace]
function eats(): void {
    $chunks = [];
    while (true) {
        $chunks[] = str_repeat('x', 1 << 20);
    }
}

eats();
