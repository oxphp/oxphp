<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A request that runs out of memory goes no further than the point it ran out
// at, even where what is freed on the way out has to wait: here, a connection
// that writes out what its filter holds as it closes, with no room left on it.
// That wait is one RUNTIME_HOOKS takes over, and it is not taken: the request
// ends in the error without waiting for room.
//
// From PHP 8.4.27 / 8.5.12 the engine unwinds the code that ran out of memory
// with the exception exit() raises, and keeps a note that it has reported the
// error until the request is shut down. A request that parked on its way out
// gave up that exception to the thread, and resumed past the frame it was
// leaving as if the call had returned. With the note it still ended in the
// error once its handler returned; without it, it answered as if nothing had
// happened.
//
// This request is parked while the inner one runs: PHP_WORKERS=1 in this profile.

$t = new TestCase('oom_while_a_connection_closes_does_not_run_on', 'fibers');

$run = bin2hex(random_bytes(6));
$marker = sys_get_temp_dir() . '/oxphp-oom-close-' . $run;

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 20);
fwrite($sock, "GET /tests/fibers/fixture_oom_while_its_connection_closes.php?run=$run HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
$resp = (string) stream_get_contents($sock);
$answered = microtime(true);
fclose($sock);
$t->meta('response', $resp);

$mark = is_file($marker) ? (string) file_get_contents($marker) : '';
$t->meta('marker', $mark);

// What the rest stands on: the inner request got as far as running out of
// memory, holding a connection with no room left on it.
$t->assertTrue('the connection had no room left when memory ran out', str_starts_with($mark, 'full '));

// From the moment it started to run out of memory to its answer. Waiting for
// room would take the connection's 3-second timeout.
$ranOutAt = (float) (explode(' ', $mark)[1] ?? 0);
$t->meta('answered after running out', round($answered - $ranOutAt, 3));
$t->assertTrue('the close did not wait for room', $answered - $ranOutAt < 1.5);
$t->assertContains('the request answers 500', $resp, ' 500 ');
$t->assertTrue('and nothing after the closure ran', !str_contains($mark, 'ran on') && !str_contains($resp, 'RAN-ON'));

if (is_file($marker)) {
    unlink($marker);
}

$t->done();
