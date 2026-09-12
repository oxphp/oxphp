<?php
// Calls back into this same server over HTTP. The outer request holds a worker
// for as long as the inner one takes, so the inner request can only be served
// once the pool frees a worker — which it cannot do until the outer one ends.
//
// Reports what the inner call came *back* as, not only how long it took. A wait
// that ends in a 529 is the queue answering on its deadline; a wait that ends in
// a stream timeout is the queue never answering at all. From the outside both
// look like "the request was slow", and telling them apart is the whole point
// of the scenarios that use this file.
// The port the server listens on *inside* the container, not the one the test
// reached it on. $_SERVER['SERVER_PORT'] is taken from the Host header, so it
// carries the published port — connecting back to that from in here is refused,
// which the fixture would then report as the timeout it exists to distinguish.
$port = isset($_GET['p']) ? (int) $_GET['p'] : 80;
$timeout = isset($_GET['t']) ? (float) $_GET['t'] : 20.0;
// Milliseconds to hold the worker *before* calling back. A caller that fires
// the inner request immediately takes whatever queue slot is free at that
// instant, so a scenario that needs the inner call to meet a full queue has to
// let the queue fill first — the delay is what makes which of the two waits is
// under test a property of the test rather than of the scheduler.
$delay = isset($_GET['d']) ? max(0, min((int) $_GET['d'], 30000)) : 0;
usleep($delay * 1000);

// What the inner call asks for. Defaults to a handler that does nothing and
// leaves no trace, which is what the timing scenarios want; a scenario that
// needs to know whether the inner request ever *ran* points this at one that
// says so.
$inner_path = isset($_GET['i'])
	? preg_replace('#[^A-Za-z0-9._?=&/-]#', '', $_GET['i'])
	: 'pause.php?ms=0';

$ctx = stream_context_create(['http' => [
	'timeout' => $timeout,
	// Without this a 529 makes file_get_contents() return false, which is
	// exactly what a timeout does — the one distinction this fixture exists
	// to report would be erased before it could be read.
	'ignore_errors' => true,
]]);

$started = microtime(true);
@file_get_contents("http://127.0.0.1:{$port}/{$inner_path}", false, $ctx);
$waited = (int) round((microtime(true) - $started) * 1000);

$inner = 'timeout';
if (isset($http_response_header[0])
	&& preg_match('#^HTTP/\d(?:\.\d)?\s+(\d{3})#', $http_response_header[0], $m)) {
	$inner = $m[1];
}

header('Content-Type: text/plain');
echo "inner={$inner} waited={$waited}ms\n";
