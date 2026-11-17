<?php

declare(strict_types=1);

// Where a request's output started, which headers_sent($file, $line) reports,
// belongs to that request. The engine records it per thread at the first write
// of a request and never replaces it, so a request that found one left on the
// thread would report where another request started writing.
//
// A writer writes and keeps the worker busy; a reader that arrives meanwhile
// starts as the writer ends, with nothing resumed in between, and reports its
// own first write. A holder writes and parks; a neighbour sent meanwhile
// reports its own, and the holder resumes to its own.

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

$sharedState['os_holder'] = false;
$holderSock = thread_cell_send("$fixture?first=1&park=600&edge=os_holder");
$parked = thread_cell_wait_for($sharedState, 'os_holder');
$neighbour = $answer(thread_cell_read(thread_cell_send("$fixture?watch=os_holder")));
$holder = $answer(thread_cell_read($holderSock));

$t = new TestCase('output_start_stays_with_its_request', 'fibers');
$t->meta('writer', $writer);
$t->meta('reader', $reader);
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);

$t->assertTrue('the writer reports that its output started in the fixture', $writer['started_here'] ?? null);
$t->assertSame('on the line it first wrote on', $writer['started_at'] ?? null, $writer['wrote_at'] ?? 0);
$t->assertSame(
    'the reader started as the writer ended',
    $reader['premise'] ?? null,
    ['busy_done' => true, 'reader_resumed' => false]
);
$t->assertNotEqual('and first wrote on another line', $reader['wrote_at'] ?? null, $writer['wrote_at'] ?? null);
$t->assertSame('and reports that line', $reader['started_at'] ?? null, $reader['wrote_at'] ?? 0);

$t->assertTrue('the holder parked', $parked);
$t->assertSame('the neighbour ran while the holder was parked', $neighbour['watched_parked'] ?? null, true);
$t->assertSame('the neighbour reports its own first write', $neighbour['started_at'] ?? null, $neighbour['wrote_at'] ?? 0);
$t->assertSame('the holder resumes to its own', $holder['started_at'] ?? null, $holder['wrote_at'] ?? 0);

$t->done();
