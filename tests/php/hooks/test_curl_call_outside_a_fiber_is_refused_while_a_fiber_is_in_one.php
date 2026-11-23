<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';
require_once __DIR__ . '/../fibers/session_inner_request.php';

// PHP that runs in no request — here a session save handler's destructor the worker
// runs between two slices — cannot wait for a cURL handle. Made on a handle a
// parked request is in the middle of a call on, the call is refused with an
// exception, and the parked transfer goes on undisturbed; see
// fixture_curl_outside_fiber.php for the calls each variant makes.
//
//   ?via=easy   the handle is in curl_exec(): curl_setopt() on it would have the
//               rest of the answer printed instead of returned. curl_multi_exec()
//               given that easy handle is refused the same way, rather than taken
//               for a multi handle another request holds and returned from at once.
//   ?via=multi  the easy handle is in a multi handle the parked request is in
//               curl_multi_select() on: curl_setopt() on the easy handle is
//               refused as well, and curl_multi_exec() on the multi handle
//               returns CURLM_OK at once, its transfer still running.
//   ?via=member the call is on an easy handle in a multi handle no call is on,
//               added by this request, which is in curl_exec() on another handle
//               meanwhile: being in a multi handle is not being in a call, so the
//               call goes ahead.

$via = (string) ($_GET['via'] ?? 'easy');
$t = new TestCase('curl_call_outside_a_fiber_is_refused_while_a_fiber_is_in_one', 'hooks');

unset($sharedState['curl_outside'], $sharedState['curl_outside_neighbour'], $sharedState['curl_in_select']);
$holder = curl_hold_start('mode=' . ($via === 'multi' ? 'multi' : 'easy') . '&delay=3000');
if ($via === 'member') {
    $memberMh = curl_multi_init();
    $member = curl_init('http://' . net_peer_ip() . ':8080/100/member');
    curl_multi_add_handle($memberMh, $member);
    $sharedState['curl_member'] = $member;
}
$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the holding request is parked in its transfer', $parked);
if (!$parked) {
    oxphp_async_await($holder, 12.0);
    $t->done();
}

$neighbourSock = session_inner_send('/tests/hooks/fixture_curl_outside_fiber.php?role=neighbour', str_repeat('a1', 16), 8.0);
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['curl_outside_neighbour']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$sessionSock = session_inner_send('/tests/hooks/fixture_curl_outside_fiber.php?role=holder&via=' . $via, str_repeat('b2', 16), 8.0);
// Parked, so that nothing of this request is what the worker runs when the session
// holder lets go of its handler: on a timer, or with ?via=member inside curl_exec().
if ($via === 'member') {
    $mine = curl_init('http://' . net_peer_ip() . ':8080/2000/mine');
    curl_setopt_array($mine, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
    $sharedState['curl_member_adder_in_exec'] = true;
    $mineBody = curl_exec($mine);
    $sharedState['curl_member_adder_in_exec'] = false;
} else {
    oxphp_sleep(2.0);
}

$session = json_decode(session_inner_read($sessionSock, 8.0), true);
$neighbour = json_decode(session_inner_read($neighbourSock, 8.0), true);
$held = oxphp_async_await($holder, 12.0);
$outside = $sharedState['curl_outside'] ?? null;

$t->meta('outside', $outside);
$t->meta('holder', $held);
$t->assertSame('the session holder answered', $session['who'] ?? $session, 'holder');
$t->assertTrue('the neighbour let go of its handler', $neighbour['configured'] ?? false);
$t->assertTrue('the destructor ran', is_array($outside));
$t->assertTrue('while the request was parked in its transfer', $outside['curl_parked'] ?? false);

if ($via === 'member') {
    $t->assertTrue('while the request that added the handle was in curl_exec() on another one', $outside['adder_in_exec'] ?? false);
    $t->assertSame('curl_setopt() went ahead', $outside['setopt'] ?? null, 'returned true');
    $t->assertSame("and this request's curl_exec() got its whole answer", $mineBody, 'curl-done:/2000/mine');
    curl_multi_remove_handle($memberMh, $member);
    unset($sharedState['curl_member'], $sharedState['curl_member_adder_in_exec']);
} else {
    $t->assertContains('curl_setopt() was refused with an AsyncException', $outside['setopt'] ?? '', 'OxPHP\Async\AsyncException');
    $t->assertContains('naming the call the other fiber is in', $outside['setopt'] ?? '', 'another fiber is in the middle of a cURL call');
}

if ($via === 'multi') {
    $t->assertTrue('the request was in curl_multi_select() at the time', $outside['in_select'] ?? false);
    $t->assertSame('curl_multi_exec() returned CURLM_OK', $outside['multi_exec'] ?? null, 'returned ' . CURLM_OK);
    $t->assertTrue('and reported the transfer as running', ($outside['running'] ?? 0) >= 1);
} elseif ($via === 'easy') {
    $t->assertContains('curl_multi_exec() given the easy handle is refused, not returned from', $outside['multi_exec'] ?? '', 'OxPHP\Async\AsyncException');
}

$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/3000/holder');

unset($sharedState['curl_outside'], $sharedState['curl_outside_neighbour'], $sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked']);

$t->done();
