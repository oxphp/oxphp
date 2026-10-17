<?php

declare(strict_types=1);

// Inner request for the timeout/test_stream_wait_* tests.
//
// Sets a time limit of ?limit= seconds and streams 64 MiB in 64 KiB chunks, one
// flush per chunk, to a client that reads nothing. Once the buffers between them
// are full a flush has to wait for room, and the limit has to end the request
// inside that wait. What it reaches is written to the file ?token= names — how
// far it got as it goes, and how it ended from its shutdown function — since
// the test runs on another worker and cannot see this request's memory.
//
// ?throw=1: every chunk goes through an output handler of its own that throws
// in the chunk's flush, and the flush's catch takes the handler off and counts
// what it caught.

$file = sys_get_temp_dir() . '/oxphp-stream-wait-'
    . preg_replace('/[^0-9a-f]/', '', (string) ($_GET['token'] ?? '')) . '.json';
$startedAt = microtime(true);
$state = ['stage' => null, 'flushes' => 0, 'ended' => false];

// Written whole and renamed into place, so the test never reads half of it.
$publish = static function () use (&$state, $file): void {
    file_put_contents("$file.tmp", json_encode($state));
    rename("$file.tmp", $file);
};

register_shutdown_function(static function () use (&$state, $publish, $startedAt): void {
    $state['ended'] = true;
    $state['status'] = connection_status();
    $state['error'] = error_get_last()['message'] ?? null;
    $state['ran'] = microtime(true) - $startedAt;
    $publish();
});

set_time_limit((int) ($_GET['limit'] ?? 2));

header('Content-Type: application/octet-stream');

$throw = isset($_GET['throw']);
$state['thrown'] = 0;
$state['caught'] = 0;
$thrower = static function () use (&$state, $publish): string {
    $state['stage'] = 'thrown';
    $state['thrown']++;
    $publish();
    throw new RuntimeException('thrown by the output handler');
};

$chunk = str_repeat('x', 64 * 1024);
for ($i = 0; $i < 1024; $i++) {
    if ($throw) {
        ob_start($thrower);
    }
    echo $chunk;
    $state['stage'] = 'flushing';
    $publish();
    try {
        oxphp_stream_flush();
    } catch (RuntimeException $e) {
        ob_end_clean();
        $state['caught']++;
    }
    $state['stage'] = 'flushed';
    $state['flushes']++;
}

$state['stage'] = 'done';
