<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// filter_*(INPUT_GET/POST/COOKIE) must read the input of the request asking, and
// of no other request the worker has served.
//
// ext/filter does not read $_GET, $_POST or $_COOKIE. It keeps the parsed input
// in three arrays of its own, one set per thread, filled through the SAPI input
// filter every time the engine rebuilds a superglobal — and emptied by one
// function, which the engine calls from sapi_activate(). Worker mode replaces
// sapi_activate() with a per-request reset of its own, so that emptying happens
// once per worker while the filling happens once per request: the storage
// accumulates, and one client's token, password or session id is readable by
// every request the worker serves afterwards, through a public API code reaches
// for precisely because it is the safe way to read input.
//
// Four requests, four ways the storage can be read by the wrong one, all on
// one worker (PHP_WORKERS=1 in this profile):
//
//   seed    — POST with a query string and a Cookie header. Asserts its own
//             input is visible through all three entry points, which is what
//             makes the nulls the next request demands mean "cleared" rather
//             than "ext/filter reads nothing here".
//   expect  — a plain GET, no query values of its own beyond ?mode, no cookie,
//             no body. Nothing the seed sent may be readable from it.
//   suspend — the same across a suspension, in both directions: a request that
//             parks and comes back must not read what ran in its window, and
//             the request that runs in that window must not read the parked
//             one's input. A suspension with nobody else on the worker must not
//             take a request's own input away either.
//   callback— the storage must not change hands underneath a call that is
//             reading it. filter_input_array() with a definition array reads
//             the storage once per key and runs userland in between, so a
//             FILTER_CALLBACK that parks leaves that read half-done while
//             another request takes the storage over.
//
// Key names carry a prefix of their own: the storage under test is shared with
// every other test in the profile, and a bare `token` or `user` would be
// asserted absent while a neighbouring test had every right to put one there.

$t = new TestCase('filter_input_not_shared', 'hooks');

$mode = $_GET['mode'] ?? '';
$t->meta('mode', $mode);

// Plain variables rather than const: a worker keeps the constants a request
// defines for the rest of its life, and the second request through this file
// would redeclare them.
$seedGet    = 'seed-query-value';
$seedCookie = 'seed-session-id';
$seedPost   = 'seed-body-secret';

$suspendGet    = 'suspend-query-value';
$suspendCookie = 'suspend-session-id';

$callbackGet    = 'callback-query-value';
$callbackSecond = 'callback-second-value';

/** Every INPUT_GET/POST/COOKIE entry point ext/filter has, for one name. */
$readFilter = static function (int $type, string $name): array {
    return [
        'has' => filter_has_var($type, $name),
        'one' => filter_input($type, $name),
        'all' => filter_input_array($type),
    ];
};

/** This request's own input must be readable — the precondition of every
 *  negative assertion below, and the only thing that tells "the storage was
 *  cleared" apart from "the storage was never filled". */
$assertOwnInputVisible = static function (
    string $when,
    int $type,
    string $label,
    string $name,
    string $expected
) use ($t, $readFilter): void {
    $seen = $readFilter($type, $name);

    $t->assertSame("filter_has_var(INPUT_$label, $name) $when", $seen['has'], true);
    $t->assertSame("filter_input(INPUT_$label, $name) $when", $seen['one'], $expected);
    $t->assertSame(
        "filter_input_array(INPUT_$label) carries $name $when",
        is_array($seen['all']) ? ($seen['all'][$name] ?? null) : $seen['all'],
        $expected
    );
};

/** And another request's input must not be. All three entry points, because
 *  they read the storage by three different routes. */
$assertNotVisible = static function (
    string $whose,
    int $type,
    string $label,
    string $name
) use ($t, $readFilter): void {
    $seen = $readFilter($type, $name);

    $t->assertSame("filter_has_var(INPUT_$label, $name) does not see $whose", $seen['has'], false);
    $t->assertNull("filter_input(INPUT_$label, $name) does not see $whose", $seen['one']);
    if (is_array($seen['all'])) {
        $t->assertKeyMissing("filter_input_array(INPUT_$label) does not carry $whose", $seen['all'], $name);
    } else {
        $t->assertNull("filter_input_array(INPUT_$label) reads no storage at all", $seen['all']);
    }
};

if ($mode === 'seed') {
    $assertOwnInputVisible('for the request that sent it', INPUT_GET, 'GET', 'oxfilter_token', $seedGet);
    $assertOwnInputVisible('for the request that sent it', INPUT_COOKIE, 'COOKIE', 'oxfilter_sid', $seedCookie);
    $assertOwnInputVisible('for the request that sent it', INPUT_POST, 'POST', 'oxfilter_pw', $seedPost);

    $t->done();
}

if ($mode === 'suspend') {
    // Before any suspension: ordinary reads, and the baseline the two checks
    // after the sleeps are measured against.
    $assertOwnInputVisible('before suspending', INPUT_GET, 'GET', 'oxfilter_token', $suspendGet);
    $assertOwnInputVisible('before suspending', INPUT_COOKIE, 'COOKIE', 'oxfilter_sid', $suspendCookie);

    // A suspension with nobody else on the worker. Hooked: this parks the
    // request fiber and hands the thread back to the event loop, which finds
    // nothing else to run and comes back to this request. Nothing has touched
    // the storage in the window, so the request must find its own input where
    // it left it — a resume that clears unconditionally would answer null here.
    sleep(1);
    $assertOwnInputVisible('after a suspension nobody else ran in', INPUT_GET, 'GET', 'oxfilter_token', $suspendGet);
    $assertOwnInputVisible('after a suspension nobody else ran in', INPUT_COOKIE, 'COOKIE', 'oxfilter_sid', $suspendCookie);

    // And now with somebody else in it. The inner request can only be served
    // while this fiber is parked, and it reads the storage from the other side:
    // its own input must be there, this request's must not.
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue('inner self-request socket connected', $sock !== false);
    stream_set_timeout($sock, 5);
    fwrite($sock, "GET /tests/hooks/fixture_inner_filter_input.php"
        . "?tag=inner&oxfilter_inner=inner-query-value HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    sleep(2);                               // hooked: suspends this request fiber

    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    // Without this the rest proves nothing: if the inner request never ran
    // inside the parked window, no storage ever changed hands.
    $t->assertContains('inner request was served while this one was parked', $resp, 'INNER-OK');

    // What the inner request left behind is not this one's to read. Its own
    // input is gone too — the inner request's start reset gave the storage back
    // before filling it, and nothing can hand a parked request its arrays again
    // — so the answer is nothing at all rather than somebody else's input.
    $assertNotVisible("the inner request's query", INPUT_GET, 'GET', 'oxfilter_inner');
    $assertNotVisible('the seed request', INPUT_POST, 'POST', 'oxfilter_pw');
    $assertNotVisible("this request's own query, taken over in the window", INPUT_GET, 'GET', 'oxfilter_token');
    $assertNotVisible("this request's own cookie, taken over in the window", INPUT_COOKIE, 'COOKIE', 'oxfilter_sid');

    // The superglobals travel with the fiber and are unaffected by any of it —
    // both the contrast the defect is measured against and the check that this
    // request is otherwise intact after two suspensions.
    $t->assertSame('$_GET still holds this request\'s own query', $_GET['oxfilter_token'] ?? null, $suspendGet);
    $t->assertSame('$_COOKIE still holds this request\'s own cookie', $_COOKIE['oxfilter_sid'] ?? null, $suspendCookie);

    $t->done();
}

if ($mode === 'callback') {
    // The storage must not change hands while a call is reading it.
    // filter_input_array() with a definition array reads ext/filter's storage
    // through the module-globals slot itself and re-reads it for every key,
    // with userland in between whenever a key asks for FILTER_CALLBACK. So the
    // request that is asking must not be parked inside such a callback while
    // another request takes the storage over.
    //
    // The inner self-request is sent first and is what would take it: it can
    // only be served while this fiber is parked, and the only place this one
    // can park is the callback below.
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue('inner self-request socket connected', $sock !== false);
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooks/fixture_inner_filter_input.php"
        . "?tag=callback&oxfilter_inner=inner-query-value HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Two keys, and in this order: the definition array is walked in insertion
    // order, so the callback runs before the second key is looked up. One key
    // would prove nothing — the lookup that must survive the callback is the
    // one after it.
    $started = microtime(true);
    $seen = filter_input_array(INPUT_GET, [
        'oxfilter_token' => [
            'filter'  => FILTER_CALLBACK,
            'options' => static function (string $value): string {
                // Hooked, and the only place this request could park. Short on
                // purpose: under the guard it is the worker thread that waits,
                // and a wait longer than the queue's admission budget
                // (QUEUE_WAIT_TIMEOUT_MS, 1s by default) would have the inner
                // request shed with 529 before the worker could take it.
                usleep(300000);
                return $value;
            },
        ],
        'oxfilter_second' => FILTER_DEFAULT,
    ]);
    $elapsed = microtime(true) - $started;

    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    // The callback must not be able to park, and "cannot park" must mean the
    // wait happens the blocking way — not that it is skipped. A sleep that
    // returns immediately would satisfy every other assertion here while
    // silently dropping the wait an application asked for.
    $t->assertGreaterThan('the sleep inside the callback was actually waited out', $elapsed, 0.25);

    $t->assertSame(
        'the key filtered through the suspending callback is this request\'s own',
        is_array($seen) ? ($seen['oxfilter_token'] ?? null) : $seen,
        $callbackGet
    );
    $t->assertSame(
        'the key read after the callback is this request\'s own',
        is_array($seen) ? ($seen['oxfilter_second'] ?? null) : $seen,
        $callbackSecond
    );

    // The inner request runs either way — inside the callback on a build that
    // lets one park there, after the call on a build that does not. Either way
    // it must read its own input and none of this request's.
    $t->assertContains('inner request read only its own input', $resp, 'INNER-OK');

    $t->done();
}

// ?mode=expect — a plain GET with no input of its own beyond ?mode, no cookie
// header and no body. Everything the seed request sent must be unreadable.
$assertNotVisible('the seed request', INPUT_GET, 'GET', 'oxfilter_token');
$assertNotVisible('the seed request', INPUT_COOKIE, 'COOKIE', 'oxfilter_sid');
$assertNotVisible('the seed request', INPUT_POST, 'POST', 'oxfilter_pw');
$assertNotVisible('the seed request', INPUT_POST, 'POST', 'oxfilter_user');

// The positive half for GET, in this request rather than the one before it: the
// storage is live and holds this request's own query string, so the four checks
// above are reading a filled array and finding nothing of the seed's in it. GET
// only — a request with no cookie and no body has no input of its own to make
// the same point with, and the seed line one request earlier is what pins those
// two.
$t->assertSame('filter_input(INPUT_GET, mode) reads this request', filter_input(INPUT_GET, 'mode'), 'expect');

// And the contrast the defect was always measured against: the superglobals
// were never wrong, only the storage ext/filter reads beside them.
$t->assertKeyMissing('$_GET carries nothing of the seed request', $_GET, 'oxfilter_token');
$t->assertSame('$_COOKIE is empty', $_COOKIE, []);
$t->assertSame('$_POST is empty', $_POST, []);

$t->done();
