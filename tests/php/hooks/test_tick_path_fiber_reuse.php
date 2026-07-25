<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('tick_path_fiber_reuse', 'hooks');

// A new request reaches a fiber through one of two dispatch sites: the serve
// loop's fast path, taken when the worker has no live fibers, and the event
// loop's tick, taken when it has. This test covers the tick one.
//
// PHP_WORKERS=1, so both inner requests below land on this worker, and neither
// can be served until this fiber suspends in the sleep. The outer request took
// the fiber that was on the free list, so inner #1 runs on a fresh one and, by
// completing, hands it back — inner #2 then starts on a recycled fiber from
// inside the tick. A build that treats "recycled" as "resume" installs that
// fiber's never-written state snapshot over the request's own, which costs inner
// #2 its header list and status, and normally takes the worker down with a
// SIGSEGV on the second header() call.
$socks = [];
foreach ([1, 2] as $i) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("inner #$i socket connected", $sock !== false);
    stream_set_timeout($sock, 5);
    fwrite($sock, "GET /tests/hooks/fixture_inner_state.php?tag=$i HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $socks[$i] = $sock;
}

sleep(2); // hooked: suspends this fiber, so the worker runs its event loop

$bodies = [];
foreach ($socks as $i => $sock) {
    $bodies[$i] = (string) stream_get_contents($sock);
    fclose($sock);
}

foreach ([1, 2] as $i) {
    $t->assertContains("inner #$i reported correct request state", $bodies[$i], 'INNER-OK');
}

// Consecutive request indices on one worker: inner #2 was dispatched right after
// inner #1 finished, which is what puts it on the fiber inner #1 recycled.
preg_match('/"worker_id":(\d+).*"request_count":(\d+)/', $bodies[1], $m1);
preg_match('/"worker_id":(\d+).*"request_count":(\d+)/', $bodies[2], $m2);
$t->assertTrue(
    'inner requests ran back to back on one worker (worker ' . ($m1[1] ?? '?')
        . '/' . ($m2[1] ?? '?') . ', requests ' . ($m1[2] ?? '?') . ' → ' . ($m2[2] ?? '?') . ')',
    isset($m1[2], $m2[2]) && $m1[1] === $m2[1] && (int) $m2[2] === (int) $m1[2] + 1
);

$t->done();
