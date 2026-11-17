<?php

declare(strict_types=1);

// The headers of the last HTTP response a request fetched through the http://
// wrapper and the string its strtok() is splitting belong to that request. The
// engine keeps both per thread for the whole request, so a request that left
// them there would hand them to the next request on the worker, and one that
// parked would come back to whatever a neighbour fetched or split meanwhile.
//
// A leaver fetches, starts a strtok() and keeps the worker busy; a request that
// arrives meanwhile starts as the leaver ends, with nothing resumed in between,
// and finds neither.
// A holder does the same and parks; a neighbour sent meanwhile finds neither as
// it starts, fetches and splits its own, and the holder resumes to its own.
//
// ?action=trigger leaves both the same way, and ?action=check, the request
// after it, starts with nothing parked on the worker.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$nothing = ['fetched_type' => null, 'next_token' => null];

$action = (string) ($_GET['action'] ?? '');
if ($action === 'trigger') {
    $ctx = stream_context_create(['http' => ['timeout' => 3, 'ignore_errors' => true]]);
    $fetched = @file_get_contents('http://127.0.0.1:9090/healthz', false, $ctx);
    $t = new TestCase('last_http_headers_and_strtok_stay_with_their_request_trigger', 'fibers');
    $t->assertSame('the fetch got an answer', $fetched, 'liveness');
    $t->assertSame('the strtok() started', strtok('t1 t2', ' '), 't1');
    $t->done();
}
if ($action === 'check') {
    $t = new TestCase('last_http_headers_and_strtok_stay_with_their_request_check', 'fibers');
    $t->assertNull('no fetched headers as it starts', http_get_last_response_headers());
    $t->assertNull('no strtok() string to carry on', thread_cell_strtok_next());
    $t->done();
}

$fixture = '/tests/fibers/fixture_http_headers_strtok.php';
$body = static fn (array $response): array => json_decode($response['body'], true) ?? [];

$sharedState['cell_busy_done'] = false;
$sharedState['cell_reader_resumed'] = false;
$queuedPromise = thread_cell_send_later($fixture, 0.15);
$leaver = $body(thread_cell_read(thread_cell_send("$fixture?fetch=healthz&tok=l1+l2+l3&busy=500")));
$sharedState['cell_reader_resumed'] = true;
$queued = $body(oxphp_async_await($queuedPromise, 10.0));

$sharedState['hs_holder'] = false;
$holderSock = thread_cell_send("$fixture?fetch=healthz&tok=h1+h2+h3&park=600&edge=hs_holder");
$parked = thread_cell_wait_for($sharedState, 'hs_holder');
$neighbour = $body(thread_cell_read(thread_cell_send("$fixture?fetch=config&tok=n1+n2&watch=hs_holder")));
$holder = $body(thread_cell_read($holderSock));

$t = new TestCase('last_http_headers_and_strtok_stay_with_their_request', 'fibers');
$t->meta('leaver', $leaver);
$t->meta('queued', $queued);
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);

$t->assertSame(
    'the leaver fetched and started a strtok()',
    $leaver['end'] ?? null,
    ['fetched_type' => 'text/plain', 'next_token' => 'l2']
);
$t->assertSame(
    'the request queued behind it started as it ended',
    $queued['premise'] ?? null,
    ['busy_done' => true, 'reader_resumed' => false]
);
$t->assertSame('and finds neither', $queued['start'] ?? null, $nothing);

$t->assertTrue('the holder parked', $parked);
$t->assertSame('the neighbour ran while the holder was parked', $neighbour['watched_parked'] ?? null, true);
$t->assertSame("the neighbour finds neither of the holder's as it starts", $neighbour['start'] ?? null, $nothing);
$t->assertSame(
    'and has its own at its end',
    $neighbour['end'] ?? null,
    ['fetched_type' => 'application/json', 'next_token' => 'n2']
);
$t->assertSame(
    'the holder resumes to its own',
    $holder['end'] ?? null,
    ['fetched_type' => 'text/plain', 'next_token' => 'h2']
);

$t->done();
