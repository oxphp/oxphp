<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The apm profile turns on trace context. traceContext() carries the three ids
// $_SERVER does, with the parent as null rather than "" when no traceparent
// came in, and its trace id is the one the APM functions report.
$t = new TestCase('request_trace_context', 'apm');

$ctx = oxphp_http_request()->traceContext();
$t->assertInstanceOf('traceContext() is a TraceContextInterface', $ctx, \OxPHP\Http\TraceContextInterface::class);

if ($ctx !== null) {
    $t->assertSame('traceId() === OXPHP_TRACE_ID', $ctx->traceId(), $_SERVER['OXPHP_TRACE_ID'] ?? null);
    $t->assertSame('traceId() === oxphp_apm_trace_id()', $ctx->traceId(), oxphp_apm_trace_id());
    $t->assertSame('spanId() === OXPHP_SPAN_ID', $ctx->spanId(), $_SERVER['OXPHP_SPAN_ID'] ?? null);
    $t->assertMatch('spanId() is 16 hex chars', $ctx->spanId(), '/^[0-9a-f]{16}$/');

    if (($_GET['case'] ?? '') === 'incoming') {
        // Sent by the suite: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01
        $t->assertSame('traceId() continues the incoming trace', $ctx->traceId(), '0af7651916cd43dd8448eb211c80319c');
        $t->assertSame('parentSpanId() is the incoming span', $ctx->parentSpanId(), 'b7ad6b7169203331');
        $t->assertSame('parentSpanId() === OXPHP_PARENT_SPAN_ID', $ctx->parentSpanId(), $_SERVER['OXPHP_PARENT_SPAN_ID'] ?? null);
        $t->assertNotEqual('spanId() is this hop, not the incoming span', $ctx->spanId(), 'b7ad6b7169203331');
    } else {
        $t->assertNull('parentSpanId() is null without a traceparent', $ctx->parentSpanId());
        $t->assertSame('OXPHP_PARENT_SPAN_ID is "" in that case', $_SERVER['OXPHP_PARENT_SPAN_ID'] ?? null, '');
    }

    // array_walk() hands out references to the properties behind the getters.
    // They are typed, so a write of another type does not reach them. One
    // property per snapshot: the first TypeError ends the walk.
    foreach (['traceId', 'spanId', 'parentSpanId'] as $getter) {
        $walked = oxphp_http_request()->traceContext();
        $t->assertThrows("array_walk() writing an int behind $getter() throws", static function () use ($walked, $getter): void {
            array_walk($walked, static function (&$v, string $k) use ($getter): void {
                if (str_ends_with($k, "\0$getter")) {
                    $v = 1;
                }
            });
        }, \TypeError::class);
        $t->assertSame("$getter() still returns its id", $walked->$getter(), $ctx->$getter());
    }
}

$t->done();
