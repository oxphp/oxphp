<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('parallel_channel_outlives_request', 'zts_ext');

// parallel keeps named channels in one hash allocated persistently at the
// extension's MINIT and destroyed at MSHUTDOWN (src/channel.c); channel.c
// registers no RINIT and no RSHUTDOWN. On a server that table therefore lives
// for the life of the process, and a name claimed by one request is still
// claimed for every request after it.
//
// This is the extension's own design rather than anything the server does, and
// there is no reset to call: the table is file-static and the extension exposes
// no way to clear it. So the test pins the behaviour rather than asserting a
// preference — an application that treats `Channel::make($name)` as
// once-per-request is the thing that breaks, and it breaks silently.
//
// The name is unique per run so a repeat run against a still-running container
// measures the request boundary rather than the previous run's leftovers.
$name = 'oxphp-zts-ext-' . bin2hex(random_bytes(8));
$payload = 'from-the-second-request';

$channel = \parallel\Channel::make($name, \parallel\Channel::Infinite);
$t->assertNotNull('the first request created the named channel', $channel);

// Second request, same process. Traditional mode, so this is a separate script
// execution: anything it still sees crossed the request boundary.
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('second-request socket connected', $sock !== false);

stream_set_timeout($sock, 5);
$query = http_build_query(['name' => $name, 'payload' => $payload]);
fwrite($sock, "GET /tests/zts_ext/fixture_parallel_channel_make.php?{$query} HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

$resp = (string) stream_get_contents($sock);
fclose($sock);

// The name is taken as far as the second request is concerned — the channel the
// first request made is still registered.
$t->assertContains('the next request found the name already taken',
    $resp, 'MAKE=parallel\Channel\Error\Existence');

// And it is the same channel, not merely a name that collides: a value sent
// from the second request is readable here, in the request that created it.
$sent = str_contains($resp, ';SEND=ok');
$t->assertTrue('the next request could open that channel', $sent);

// Channel::recv() has no timeout, so it is only safe to call once the other
// request has said it sent something. Were this unguarded, a build where the
// channel no longer crosses the request boundary would hang here until the
// request times out and report as an error, instead of failing the assertions
// above and saying which half of the guarantee broke.
if ($sent) {
    $t->assertSame('a value sent from the next request arrives in this one',
        $channel->recv(), $payload);
} else {
    $t->assertTrue('a value sent from the next request arrives in this one'
        . ' (not reached: the next request could not open the channel)', false);
}

$t->done();
