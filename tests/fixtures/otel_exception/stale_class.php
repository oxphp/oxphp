<?php

// Regression fixture for the throw-hook stale-class window.
//
// The traditional-path structural class comes from a zend_throw_exception_hook
// snapshot that records EVERY thrown class and is NOT cleared on catch. This
// request throws AND catches one exception (StaleClassCaught) — so the snapshot
// briefly holds it — then dies from a DIFFERENT uncaught exception
// (StaleClassEscaped). Because the second throw happens with EG(exception) clear
// (the first was caught), it re-fires the hook and overwrites the snapshot, so
// the root span's exception.type must be StaleClassEscaped, never the stale
// StaleClassCaught.
//
// Unique class names keep the shared-collector grep from colliding with other
// scenarios' RuntimeException / LogicException spans.

class StaleClassCaught extends \RuntimeException {}
class StaleClassEscaped extends \RuntimeException {}

function escapeStale(): void
{
    throw new StaleClassEscaped('stale-class: escaped exception');
}

try {
    throw new StaleClassCaught('stale-class: caught and swallowed');
} catch (StaleClassCaught $e) {
    // Swallowed. The throw-hook snapshot now holds StaleClassCaught.
}

escapeStale();
