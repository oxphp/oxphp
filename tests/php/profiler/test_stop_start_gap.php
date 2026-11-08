<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// start() and stop() are the only way to carve a section out of a request. This
// probe exercises the shapes that can unbalance the function observer: start()
// inside a frame opened before it, a gap between two captured calls, and stop()
// from inside a span that is still open.
//
// profiler/test_stop_start_gap_index reads the resulting run back.

$t = new TestCase('stop_start_gap', 'profiler');

$t->assertFalse('pause() is gone', function_exists('OxPHP\Profile\pause'));
$t->assertFalse('resume() is gone', function_exists('OxPHP\Profile\resume'));

// Not constant bodies: OPcache inlines a call to a function that only returns a
// constant, and an inlined call has no frame for the observer to see.
function _gap_before(int $n): int { return $n + 1; }
function _gap_hidden(int $n): int { return $n + 2; }
function _gap_after(int $n): int { return $n + 3; }
function _gap_stops_inside(): void { OxPHP\Profile\stop(); }

function _gap_outer(): void
{
    OxPHP\Profile\start();
    _gap_before(1);
    OxPHP\Profile\stop();
    _gap_hidden(1);
    OxPHP\Profile\start();
    _gap_after(1);
    _gap_stops_inside();
}

_gap_outer();

$t->assertFalse('stop() from a deeper span ends capture', OxPHP\Profile\is_active());

$t->done();
