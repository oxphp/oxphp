<?php
// Callback path: oxphp_apm_trace() runs the callback inside a child span whose
// whole lifetime the function owns. Two spans leave this request — one whose
// callback returns, one whose callback throws.
//
// The throwing span carries exception.type and exception.message but NO
// exception.stacktrace: unlike oxphp_apm_error(), this path runs on every throw
// inside a traced callback, so it does not re-enter PHP for getTraceAsString().

$sum = oxphp_apm_trace('trace_cb.ok', function (int $span): int {
    // Addressing the span by the id the callback was handed — not by "current"
    // — is what proves the id identifies this span and not some other one.
    oxphp_apm_attribute('trace_cb.inner', 'set-from-callback', $span);
    return 40 + 2;
}, ['component' => 'trace-cb']);

echo "trace_cb returned $sum\n";

try {
    oxphp_apm_trace('trace_cb.boom', function (int $span): void {
        throw new RangeException('trace cb path: out of range');
    });
    echo "trace_cb swallowed the exception\n";
} catch (\RangeException $e) {
    echo "trace_cb rethrew: {$e->getMessage()}\n";
}

// PHP's two throwable hierarchies each declare their own protected $message,
// and neither derives from the other. A helper that reads it with Exception as
// the property scope comes back empty for every Error — TypeError, ValueError,
// DivisionByZeroError — and the attribute is then dropped outright. The
// RangeException above cannot show that; this can.
try {
    oxphp_apm_trace('trace_cb.error_hierarchy', function (int $span): void {
        throw new TypeError('trace cb path: wrong type');
    });
} catch (\TypeError $e) {
    echo "trace_cb rethrew error-hierarchy: {$e->getMessage()}\n";
}
