<?php

declare(strict_types=1);

// Whether a request's output has gone out belongs to that request. PHP keeps
// it per thread and, once it is set, refuses with a warning a change to
// zlib.output_compression — the change header('Content-Length: …') makes too —
// so a request that found it left on the thread would be refused before it
// wrote anything.
//
// A writer writes and keeps the worker busy; a reader that arrives meanwhile
// starts as the writer ends, with nothing resumed in between. A writer writes
// and parks; a neighbour sent meanwhile starts with nothing sent, and the
// writer resumes to its own output sent. A silent holder parks before writing;
// a talker writes and ends meanwhile, and the holder resumes with nothing sent.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$fixture = '/tests/fibers/fixture_output_start.php';
$answer = static function (array $response): array {
    $lines = explode("\n", rtrim($response['body']));

    return json_decode((string) end($lines), true) ?? [];
};

$sharedState['cell_busy_done'] = false;
$sharedState['cell_reader_resumed'] = false;
$readerPromise = thread_cell_send_later($fixture, 0.15);
$writer = $answer(thread_cell_read(thread_cell_send("$fixture?first=1&busy=500")));
$sharedState['cell_reader_resumed'] = true;
$reader = $answer(oxphp_async_await($readerPromise, 10.0));

$sharedState['sent_writer'] = false;
$writerSock = thread_cell_send("$fixture?first=1&park=600&edge=sent_writer");
$writerParked = thread_cell_wait_for($sharedState, 'sent_writer');
$neighbour = $answer(thread_cell_read(thread_cell_send("$fixture?watch=sent_writer")));
$parkedWriter = $answer(thread_cell_read($writerSock));

$sharedState['sent_silent'] = false;
$silentSock = thread_cell_send("$fixture?park=600&edge=sent_silent");
$silentParked = thread_cell_wait_for($sharedState, 'sent_silent');
$talker = $answer(thread_cell_read(thread_cell_send("$fixture?first=1&watch=sent_silent")));
$silent = $answer(thread_cell_read($silentSock));

$t = new TestCase('output_sent_stays_with_its_request', 'fibers');
$t->meta('writer', $writer);
$t->meta('reader', $reader);
$t->meta('neighbour', $neighbour);
$t->meta('parked_writer', $parkedWriter);
$t->meta('talker', $talker);
$t->meta('silent', $silent);

$t->assertTrue('zlib is loaded', $writer['zlib'] ?? null);
$t->assertTrue('the writer can change zlib.output_compression before it writes', $writer['changeable_at_start'] ?? null);
$t->assertSame('and not after', $writer['changeable_after_park'] ?? null, false);
$t->assertSame(
    'the reader started as the writer ended',
    $reader['premise'] ?? null,
    ['busy_done' => true, 'reader_resumed' => false]
);
$t->assertTrue('and can change it', $reader['changeable_at_start'] ?? null);

$t->assertTrue('the parking writer parked', $writerParked);
$t->assertSame('the neighbour ran while it was parked', $neighbour['watched_parked'] ?? null, true);
$t->assertTrue('and can change it', $neighbour['changeable_at_start'] ?? null);
$t->assertSame('the writer resumes to its own output sent', $parkedWriter['changeable_after_park'] ?? null, false);

$t->assertTrue('the silent holder parked', $silentParked);
$t->assertSame('the talker ran while it was parked', $talker['watched_parked'] ?? null, true);
$t->assertSame('and wrote', $talker['changeable_after_park'] ?? null, false);
$t->assertTrue('the silent holder resumes with nothing sent', $silent['changeable_after_park'] ?? null);

$t->done();
