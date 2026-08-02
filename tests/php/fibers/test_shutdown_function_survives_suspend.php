<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// register_shutdown_function() files the function in a registry the engine keeps
// per thread, and the end of a request runs everything standing in that registry
// and then frees the lot. Neither half of that survives a suspension on its own:
// a request the worker serves while another is parked runs the parked request's
// shutdown functions — inside the wrong request, echoing into the wrong response
// — and frees them, so the parked request resumes with nothing left to run at
// its own end.
//
// Two self-requests, both served by this worker: the first parks in a hooked
// sleep holding a registration, the second registers one of its own and ends
// inside that window. Each function echoes the id of the request that registered
// it, so the two bodies say exactly who ran what.

$sockA = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$sockB = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$connected = $sockA !== false && $sockB !== false;

if ($connected) {
    stream_set_timeout($sockA, 10);
    stream_set_timeout($sockB, 10);
    // Written in this order and served in it: the parking one first, so the
    // other one is what the worker picks up while it is parked.
    fwrite($sockA, "GET /tests/fibers/fixture_shutdown_parks.php?id=1&park=1 HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    fwrite($sockB, "GET /tests/fibers/fixture_shutdown_parks.php?id=2 HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
}

// Hooked: parks this request's fiber, and for longer than the first inner
// request stays parked, so both of them have finished by the reads below.
// Without the park there is no worker to serve either — this profile runs one.
sleep(2);

$respA = $connected ? (string) stream_get_contents($sockA) : '';
$respB = $connected ? (string) stream_get_contents($sockB) : '';
if ($connected) {
    fclose($sockA);
    fclose($sockB);
}

$t = new TestCase('shutdown_function_survives_suspend', 'fibers');

$t->assertTrue('both inner self-request sockets connected', $connected);
$t->assertContains('the first inner request parked and resumed', $respA, 'RESUMED:1');
$t->assertContains('the second inner request ran inside that window', $respB, 'ARMED:2');

$t->assertContains(
    'a request runs its own shutdown functions into its own response',
    $respB,
    'SHUTDOWN-RAN:2'
);

$t->assertNotContains(
    'and does not run the shutdown functions of the request parked meanwhile',
    $respB,
    'SHUTDOWN-RAN:1'
);

$t->assertContains(
    'which is still there to run when that request resumes and ends',
    $respA,
    'SHUTDOWN-RAN:1'
);

$t->done();
