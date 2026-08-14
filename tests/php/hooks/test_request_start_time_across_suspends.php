<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The start time of a request must survive every suspension it takes, and must
// survive them whatever the neighbours are doing.
//
// It is held in one slot per worker thread rather than one per request, which
// gives the defect two shapes. If the worker serves a neighbour whole inside the
// window, that neighbour's end ERASES the slot and the resumed request reads
// 0.0. If the neighbour is still in flight, nothing has erased anything and the
// resumed request reads the NEIGHBOUR'S start time — wrong, but plausible enough
// to survive a check that only looks for zero. The companion test
// hooks/request_object_survives_suspend covers the first shape; this one covers
// the second, and covers a request that suspends more than once.
//
// PHP_WORKERS=1, so every inner request below can only be served while this
// fiber is parked.

$t = new TestCase('request_start_time_across_suspends', 'hooks');

$request = oxphp_http_request();

$t->assertGreaterThan('this request has a start time of its own', $request->startTime(true), 0.0);
$own = $request->startTime(true);

// Parks for 3s — longer than both of this request's own suspensions below — so
// it is still in flight when this request wakes from either of them.
$slow = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('slow neighbour socket connected', $slow !== false);
stream_set_timeout($slow, 10);
fwrite($slow, "GET /tests/hooks/fixture_inner_slow_start.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// ── First suspension: a neighbour is parked, none has ended ──────────────

sleep(1);                                   // hooked: suspends this request fiber
$resumedFirstAt = microtime(true);

// Captured at the resume, not at the end of the test: this is the only moment
// an inherited neighbour time can be observed. Later on the neighbour has
// finished, which erases the slot — so a check made down there would see 0 and
// "differ from the neighbour" for the wrong reason.
$afterFirst = $request->startTime(true);

$t->assertSame('startTime() survived the first suspend', $request->startTime(true), $own);
$t->assertSame('oxphp_server_info() survived the first suspend',
    oxphp_server_info()['request_time'], $own);
$t->assertSame('startTime() and $_SERVER[REQUEST_TIME] agree after the first suspend',
    $request->startTime(), (int) ($_SERVER['REQUEST_TIME'] ?? -1));

// ── Second suspension: this time a neighbour also ends inside the window ──
// A request that carries its start time across one suspension but drops it on
// the next — restoring once, or clearing the parked copy on restore — passes
// everything above and fails here.

$fast = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fast neighbour socket connected', $fast !== false);
stream_set_timeout($fast, 5);
fwrite($fast, "GET /tests/hooks/fixture_inner_state.php?tag=second-window HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

sleep(1);                                   // hooked: suspends this request fiber again

$fastResp = (string) stream_get_contents($fast);
fclose($fast);
$t->assertContains('a neighbour was served whole inside the second window',
    $fastResp, 'INNER-OK');

$t->assertSame('startTime() survived the second suspend', $request->startTime(true), $own);
$t->assertSame('oxphp_server_info() survived the second suspend',
    oxphp_server_info()['request_time'], $own);
$t->assertSame('startTime() and $_SERVER[REQUEST_TIME] agree after the second suspend',
    $request->startTime(), (int) ($_SERVER['REQUEST_TIME'] ?? -1));

// ── Now collect the slow neighbour and check what it was ─────────────────
// Read last, because reading it waits for it to finish — which is the one thing
// that must not have happened before the assertions above.

$slowResp = (string) stream_get_contents($slow);
fclose($slow);
$t->assertContains('slow neighbour served a full response', $slowResp, 'SLOW-DONE');

$parts = explode("\r\n\r\n", $slowResp, 2);
$body = json_decode($parts[1] ?? '', true);
$t->assertType('slow neighbour reported its timings', $body, 'array');
$slowStart = (float) ($body['start'] ?? 0.0);
$slowEnd   = (float) ($body['end'] ?? 0.0);

// The precondition the whole case rests on: the slow neighbour was still
// running when this request woke from its first suspension. Without this the
// assertions above are the same ones the companion test already makes.
$t->assertGreaterThan('slow neighbour was still in flight at the first resume',
    $slowEnd, $resumedFirstAt);

// And the point: waking with that neighbour parked did not hand this request the
// neighbour's start time. Compared against the reading taken at the resume
// itself — the two are real clock readings milliseconds apart, so this is the
// assertion that separates "kept its own" from "inherited", and no zero can
// satisfy it by accident.
$t->assertGreaterThan('slow neighbour has a start time of its own', $slowStart, 0.0);
$t->assertNotEqual('the first resume did not inherit the parked neighbour\'s start time',
    $afterFirst, $slowStart);
$t->assertSame('startTime() is still this request\'s own after every suspend',
    $request->startTime(true), $own);

$t->done();
