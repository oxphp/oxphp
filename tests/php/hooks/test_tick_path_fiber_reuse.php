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

// What this proves and what it does not. There is no PHP-visible fiber identity,
// so neither the fixture nor this test can assert "ran on a recycled fiber" the
// way the fast-path test asserts "not a worker's first request". The pairing
// below shows only that nothing was served between the two inner requests. Reuse
// itself rests on the accept loop in oxphp_scheduler_tick: it drains the queue in
// one pass, so inner #1 runs to completion and reaches the free list before
// inner #2 is created. That is a property of the loop, not of this test — put a
// suspend point in fixture_inner_state.php and inner #1 would still be in flight
// when inner #2 starts, which would leave this test passing while covering
// nothing. Keep the fixture free of awaits, sleeps and socket reads.
preg_match('/"worker_id":(\d+).*"request_count":(\d+)/', $bodies[1], $m1);
preg_match('/"worker_id":(\d+).*"request_count":(\d+)/', $bodies[2], $m2);
$t->assertTrue(
    'inner requests ran back to back on one worker (worker ' . ($m1[1] ?? '?')
        . '/' . ($m2[1] ?? '?') . ', requests ' . ($m1[2] ?? '?') . ' → ' . ($m2[2] ?? '?') . ')',
    isset($m1[2], $m2[2]) && $m1[1] === $m2[1] && (int) $m2[2] === (int) $m1[2] + 1
);

$t->done();
