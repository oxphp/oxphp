<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request that suspends must come back to its own superglobals.
//
// Userland does not read PG(http_globals); it reads EG(symbol_table), where the
// auto-global callbacks leave a separate reference. Every new request rebinds
// those entries to its own arrays, so a fiber parked in a hooked sleep used to
// resume reading the parameters, cookies and headers of whichever request the
// worker served in the meantime — one client's request data handed to another.
//
// PHP_WORKERS=1, so the inner self-request below can only be served while this
// fiber is suspended, which is exactly the window the defect needs. The inner
// request carries a different query string and a cookie this one does not have,
// so a bleed shows up as the intruder's values rather than as emptiness.

$t = new TestCase('superglobals_survive_suspend', 'hooks');

$t->assertSame('outer request has its own probe in $_GET', $_GET['probe'] ?? null, 'outer');

// Writes the handler makes itself have to survive the suspend too, and they are
// a separate case from the bleed. A userland write to a superglobal separates
// the array by COW — the entry is shared with the engine's own
// PG(http_globals) slot, so its refcount is at least two — which leaves the
// written copy in EG(symbol_table) and the untouched one in the slot. Anything
// that restores the symbol table *from* that slot therefore rolls the request's
// own writes back, silently and even when no other request ever ran.
$_GET['written_by_handler'] = 'kept';
$_SERVER['OX_HANDLER_MARK'] = 'kept';

$before = [
    'get'     => $_GET,
    'post'    => $_POST,
    'cookie'  => $_COOKIE,
    'request' => $_REQUEST,
    'uri'     => $_SERVER['REQUEST_URI'] ?? null,
];

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);
fwrite($sock, "GET /tests/hooks/fixture_inner_state.php?tag=intruder&probe=inner HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nCookie: ox_intruder=1\r\nConnection: close\r\n\r\n");

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Without this the rest proves nothing: if the intruder never ran inside the
// suspended window, the superglobals were never at risk in the first place.
$t->assertContains('intruder was served while this request was parked', $resp, 'INNER-OK');
$t->assertContains('intruder ran with its own query string', $resp, '"tag":"intruder"');

$t->assertSame('$_GET survived the suspend', $_GET, $before['get']);
$t->assertSame('$_POST survived the suspend', $_POST, $before['post']);
$t->assertSame('$_COOKIE survived the suspend', $_COOKIE, $before['cookie']);
$t->assertSame('$_REQUEST survived the suspend', $_REQUEST, $before['request']);
$t->assertSame('$_SERVER[REQUEST_URI] survived the suspend',
    $_SERVER['REQUEST_URI'] ?? null, $before['uri']);

// Named separately from the whole-array checks: a restore that reads the
// engine's slots rather than the symbol table loses these two and nothing else,
// so they say which of the two defects is live.
$t->assertSame('$_GET write made before the suspend survived it',
    $_GET['written_by_handler'] ?? null, 'kept');
$t->assertSame('$_SERVER write made before the suspend survived it',
    $_SERVER['OX_HANDLER_MARK'] ?? null, 'kept');

// These are the values the intruder would have left behind, so a failure here
// says what leaked.
$t->assertSame('$_GET[probe] is still this request\'s', $_GET['probe'] ?? null, 'outer');
$t->assertKeyMissing('$_GET did not pick up the intruder\'s tag', $_GET, 'tag');
$t->assertKeyMissing('$_COOKIE did not pick up the intruder\'s cookie', $_COOKIE, 'ox_intruder');

$t->done();
