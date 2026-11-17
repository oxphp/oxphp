<?php

declare(strict_types=1);

// ob_implicit_flush() is for the request that called it. The engine keeps it as
// a flag on the thread, and with it on flushes every write, so a request that
// found it left on would push each write of its event stream to its client at
// once, and one that parked could come back to it turned off by a neighbour.
//
// A setter turns it on and keeps the worker busy; an event stream that arrives
// meanwhile starts as the setter ends, with nothing resumed in between, writes
// a frame and parks, and nothing reaches its client before that park ends. A
// holder turns it on and parks;
// a neighbour sent meanwhile turns it off; the holder then writes a frame and
// parks again, and that frame reaches its client during the park.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$fixture = '/tests/fibers/fixture_implicit_flush.php';

// What reaches $sock within $window seconds, read without waiting for the
// rest, and whether the park $edge names was still in progress once it was
// read.
$early = static function ($sock, array &$state, string $edge, float $window): array {
    stream_set_blocking($sock, false);
    $got = '';
    $deadline = microtime(true) + $window;
    while ($got === '' && microtime(true) < $deadline) {
        $got .= (string) fread($sock, 65536);
        if ($got === '') {
            oxphp_usleep(20_000);
        }
    }
    $parked = ($state[$edge] ?? false) === true;
    stream_set_blocking($sock, true);

    return ['got' => $got, 'still_parked' => $parked];
};
$rest = static function ($sock, string $got): string {
    stream_set_timeout($sock, 8);
    $raw = $got . (string) stream_get_contents($sock);
    fclose($sock);

    return $raw;
};

$sharedState['cell_busy_done'] = false;
$sharedState['cell_reader_resumed'] = false;
$streamerPromise = thread_cell_send_later("$fixture?sse=1&park=600&edge=if_streamer", 0.15);
$setter = thread_cell_read(thread_cell_send("$fixture?set=on&busy=500"));
$sharedState['cell_reader_resumed'] = true;
$streamerAnswer = oxphp_async_await($streamerPromise, 10.0);
$streamer = $streamerAnswer['body'];
$streamerLast = preg_match('/data: (\{.*\})\n\n$/', $streamer, $m) === 1 ? (json_decode($m[1], true) ?? []) : [];

$sharedState['if_holder_pre'] = false;
$sharedState['if_holder'] = false;
$holderSock = thread_cell_send("$fixture?sse=1&set=on&prepark=500&preedge=if_holder_pre&park=1000&edge=if_holder");
$holderPreParked = thread_cell_wait_for($sharedState, 'if_holder_pre');
$neighbour = thread_cell_read(thread_cell_send("$fixture?set=off&watch=if_holder_pre"));
$holderParked = thread_cell_wait_for($sharedState, 'if_holder');
$holderEarly = $early($holderSock, $sharedState, 'if_holder', 0.4);
$holder = $rest($holderSock, $holderEarly['got']);

$t = new TestCase('implicit_flush_stays_with_its_request', 'fibers');
$t->meta('setter', $setter);
$t->meta('streamer_first_byte_at', $streamerAnswer['first_at']);
$t->meta('streamer', $streamer);
$t->meta('neighbour', $neighbour);
$t->meta('holder_early', $holderEarly);
$t->meta('holder', $holder);

$t->assertContains('the setter answered', $setter['body'], 'data: one');
$t->assertSame(
    'the stream started as the setter ended',
    $streamerLast['premise'] ?? null,
    ['busy_done' => true, 'reader_resumed' => false]
);
$t->assertType('and parked', $streamerLast['park_ended_at'] ?? null, 'double');
$t->assertGreaterThan(
    'nothing reached its client before that park ended',
    (float) ($streamerAnswer['first_at'] ?? 0.0),
    (float) ($streamerLast['park_ended_at'] ?? INF)
);
$t->assertContains('its frames reached it at its end', $streamer, 'data: one');

$t->assertTrue('the holder parked before writing', $holderPreParked);
$t->assertContains('the neighbour ran while the holder was parked', $neighbour['body'], '"watched_parked":true');
$t->assertTrue('the holder parked again after writing', $holderParked);
$t->assertTrue('and was still parked after the wait', $holderEarly['still_parked']);
$t->assertContains("the holder's frame reached its client during that park", $holderEarly['got'], 'data: one');
$t->assertContains('and the rest at its end', $holder, '"watched_parked":null');

$t->done();
