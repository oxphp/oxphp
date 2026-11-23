<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';
require_once __DIR__ . '/curl_shared_client.php';

// An HTTP client every request on the worker shares, with a flag over the whole
// of its perform() that fails whoever finds it raised (curl_shared_client.php).
// A holder request drives its transfer with the client's loop and is in
// curl_multi_select() when this request reaches the client. Neither request may
// be suspended inside perform(), or the other one fails:
//
//   ?arrive=perform  this request's transfer is already in the multi handle, and
//                    it calls perform(). Its curl_multi_exec() returns at once
//                    instead of waiting for the holder's select, leaving the
//                    transfers to the next curl_multi_exec(), and reports the
//                    handle's transfers as running.
//   ?arrive=add      this request adds its transfer while the holder is in the
//                    select. The add ends the select early; the holder's next
//                    curl_multi_exec(), inside its perform(), does not let the add
//                    in first — its next curl_multi_select() does.
//
// Both transfers end with their whole answer, no perform() finds the flag raised,
// and no two requests' perform() calls overlap.

$arrive = (string) ($_GET['arrive'] ?? 'perform');
$t = new TestCase('curl_shared_client_does_not_reenter_perform', 'hooks');

unset($sharedState['curl_in_select']);
$client = new OxphpSharedCurlClient();
$client->probe = static function () use (&$sharedState): bool {
    return (bool) ($sharedState['curl_in_select'] ?? false);
};
$sharedState['curl_client'] = $client;

$mine = curl_init('http://' . net_peer_ip() . ':8080/1500/visitor');
curl_setopt_array($mine, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
if ($arrive === 'perform') {
    curl_multi_add_handle($client->mh, $mine);
}

$holder = curl_hold_start('delay=2500', 'fixture_curl_shared_client.php');

$deadline = microtime(true) + 3.0;
while (!($sharedState['curl_in_select'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
oxphp_usleep(100_000);
$inSelect = (bool) ($sharedState['curl_in_select'] ?? false);
$t->assertTrue('the holder is in curl_multi_select() on the shared handle', $inSelect);
if (!$inSelect) {
    oxphp_async_await($holder, 12.0);
    unset($sharedState['curl_client'], $sharedState['curl_in_select']);
    $t->done();
}

if ($arrive === 'perform') {
    // The perform() measured below, made while the holder is in its select.
    $client->perform('visitor');

    // Without its by-reference argument the call is ext/curl's to refuse, also
    // while another request holds the handle; it waits for that request's call,
    // and so comes after the measured one.
    $refused = 'not refused';
    try {
        curl_multi_exec($client->mh);
    } catch (\ArgumentCountError $e) {
        $refused = 'ArgumentCountError';
    }
    $t->assertSame('curl_multi_exec() without $still_running is still an ArgumentCountError', $refused, 'ArgumentCountError');
} else {
    $addStarted = microtime(true);
    $added = curl_multi_add_handle($client->mh, $mine);
    $addTook = microtime(true) - $addStarted;
}

$failed = $client->run('visitor', $mine);
$body = curl_multi_getcontent($mine);
curl_multi_remove_handle($client->mh, $mine);
$held = oxphp_async_await($holder, 12.0);

$t->meta('holder', $held);
$t->meta('performs', $client->performs);
$t->meta('execs', $client->execs);
$t->meta('selects', $client->selects);
$t->meta('reentries', $client->reentries);

if ($arrive === 'perform') {
    $first = null;
    foreach ($client->execs as $exec) {
        if ($exec[0] === 'visitor') {
            $first = $exec;
            break;
        }
    }
    $t->assertSame('its first curl_multi_exec() returned CURLM_OK', $first[1] ?? null, CURLM_OK);
    $t->assertTrue('while the holder was still in its curl_multi_select()', $first[4] ?? false);
    $t->assertTrue('and reported the handle\'s transfers as running', ($first[2] ?? 0) >= 1);
} else {
    $t->assertSame('the add went in', $added, CURLM_OK);
    $t->assertLessThan('without waiting out the holder\'s select', $addTook, 0.8);
    $ended = array_filter(
        $client->selects,
        static fn (array $s): bool => $s[0] === 'holder' && $s[1] < $addStarted && $s[2] >= $addStarted && $s[2] - $s[1] < 0.9
    );
    $t->assertTrue('because the add ended the select the holder was in', $ended !== []);
}

$t->assertSame('no perform() found another one under way', $client->reentries, []);

$overlaps = [];
foreach ($client->performs as $i => [$who1, $from1, $to1]) {
    foreach ($client->performs as $j => [$who2, $from2, $to2]) {
        if ($i < $j && $who1 !== $who2 && $from1 < $to2 && $from2 < $to1) {
            $overlaps[] = [$who1, $from1, $to1, $who2, $from2, $to2];
        }
    }
}
$t->assertSame('no two requests were inside perform() at once', $overlaps, []);

$t->assertSame('this request\'s transfer did not fail', $failed, null);
$t->assertSame('and got its whole answer', $body, 'curl-done:/1500/visitor');
$t->assertSame('the holder\'s transfer did not fail', is_array($held) && array_key_exists('failed', $held) ? $held['failed'] : $held, null);
$t->assertSame('and got its whole answer', $held['body'] ?? $held, 'curl-done:/2500/holder');

curl_multi_close($client->mh);
unset($sharedState['curl_client'], $sharedState['curl_in_select']);

$t->done();
