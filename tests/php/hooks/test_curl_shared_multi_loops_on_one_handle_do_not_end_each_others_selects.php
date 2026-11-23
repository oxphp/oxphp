<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_multi_loops_on_one_handle_do_not_end_each_others_selects', 'hooks');

// Two requests each run a curl_multi_exec()/curl_multi_select() loop on one shared
// multi handle, as two requests streaming responses from a shared Symfony
// HttpClient do, with nothing to read for 3 s, and each loop suspends for 20 ms
// between two of its calls, as a client does while its callbacks wait on I/O:
// after curl_multi_select(), or with ?arrive=select after curl_multi_exec(), as
// Guzzle and Symfony HttpClient run theirs, so that the call each loop comes back
// with is a curl_multi_select(). A loop's own calls do not end the select the
// other loop is in — its curl_multi_select() waits for it, its curl_multi_exec()
// returns at once: a curl_multi_select() returns when a transfer of either request
// has data, so ending it gains nothing, and if each loop ended the other's, the two
// would hand the handle back and forth every 20 ms, neither waiting for data. Nor
// does a loop's curl_multi_select() end the other's: the select it ends would only
// be traded for its own.
$afterExec = ($_GET['arrive'] ?? '') === 'select';
$peer = net_peer_ip();
$holder = curl_hold_start('mode=multi&delay=3000&select=1&yield=' . ($afterExec ? 'exec' : '1'));

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$t->assertTrue('the holding request reached its curl_multi_select() loop and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$mh = $sharedState['curl_mh'];
$ch = curl_init("http://{$peer}:8080/3000/loop");
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$added = curl_multi_add_handle($mh, $ch);
$addedAt = microtime(true);

$codes = [];
$selects = [];
$deadline = microtime(true) + 8.0;
while ($added === CURLM_OK && microtime(true) < $deadline) {
    $codes[curl_multi_exec($mh, $running)] = true;
    if (curl_multi_getcontent($ch) === 'curl-done:/3000/loop') {
        break;
    }
    if ($afterExec) {
        usleep(20_000);
    }
    $selectStarted = microtime(true);
    $selected = curl_multi_select($mh, 1.0);
    $selects[] = [$selectStarted, microtime(true), $selected];
    if (!$afterExec) {
        usleep(20_000);
    }
}
$body = curl_multi_getcontent($ch);
$removeAt = microtime(true);
$removed = $added === CURLM_OK ? curl_multi_remove_handle($mh, $ch) : null;

$held = oxphp_async_await($holder, 12.0);
$heldSelects = count($held['selects'] ?? []);

// A select that returned 0 well before its 1 s timeout was ended by another call.
// Counted between this request's add and the first call that may end one: its own
// curl_multi_remove_handle(), or the holder's, once its loop is over.
$until = min($removeAt, $held['finished'] ?? $removeAt);
$ended = [];
foreach (['this request' => $selects, 'the holder' => $held['selects'] ?? []] as $who => $list) {
    foreach ($list as [$from, $to, $returned]) {
        if ($from >= $addedAt && $to < $until && $returned === 0 && $to - $from < 0.5) {
            $ended[] = sprintf('%s: %.3f s in', $who, $to - $from);
        }
    }
}

$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertTrue(sprintf("this request's loop made a handful of curl_multi_select() calls over 3 s (%d)",
        count($selects)), count($selects) >= 1 && count($selects) <= 20);
$t->assertTrue(sprintf("and so did the holder's (%d)", $heldSelects), $heldSelects >= 1 && $heldSelects <= 20);
$t->assertTrue(sprintf('no curl_multi_select() of either loop was ended before its timeout (%s)',
        $ended === [] ? 'none' : count($ended) . ': ' . implode(', ', array_slice($ended, 0, 3))), $ended === []);
$t->assertSame("every one of this request's curl_multi_exec() calls succeeded", array_keys($codes), [CURLM_OK]);
$t->assertSame("this request's transfer completed", $body, 'curl-done:/3000/loop');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/3000/holder');
$t->assertSame("and every one of the holder's curl_multi_exec() calls succeeded", $held['codes'] ?? null, [CURLM_OK]);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked'],
    $sharedState['curl_in_select'], $sharedState['curl_select_started']);

$t->done();
