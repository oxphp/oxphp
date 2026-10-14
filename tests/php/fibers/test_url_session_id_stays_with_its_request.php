<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A session id carried in URLs — session.use_trans_sid on and
// session.use_only_cookies off, deprecated since PHP 8.4 — stays with the
// request that started the session: in the links its response prints and in
// the SID constant it reads.
//
// The module hands the id to PHP's URL rewriter, an output handler that
// rewrites links as the response is flushed, and keeps what it appends — and
// whether the handler is already running — in state that is one per thread.
// SID is a constant, also one per thread. Neither is part of a request's
// session as the module stores it. Left on the thread, the id a request
// started in the window is the one a parked request's links are rewritten
// with once it resumes and ends — a link that signs its client in as the
// other one — and the request that started it gets no rewriting at all,
// because the handler counts as running already. Left after the request, the
// same holds for every later request on the worker, and SID names the last
// session started on it in requests that started none.
//
// A request starts its session, prints a link and parks. A request that opens
// no session reads SID in the window; another starts a session of its own and
// ends. The first resumes and ends. After both, a third starts a session, and
// one more reads SID.

$t = new TestCase('url_session_id_stays_with_its_request', 'fibers');

/** The value the link in $body was rewritten with, or null. */
$linkId = static function (string $body): ?string {
    return preg_match('~href="/next\.php\?OXRWSID=([^"&]+)"~', $body, $m) ? $m[1] : null;
};
/** What the request reported in its <p>. */
$report = static function (string $body): array {
    return preg_match('~<p>(.*)</p>~', $body, $m) ? (json_decode($m[1], true) ?: []) : [];
};

unset($sharedState['url_rewriter_parked']);
// The session cookie these send is not the one the fixture's session reads.
$parkedSock = session_inner_send('/tests/fibers/fixture_url_rewriter.php?role=trans&park=0.8', 'none', 8.0);
// Waiting parks this request, which lets the worker take the first one.
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['url_rewriter_parked']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$windowSid = json_decode(session_inner_request('/tests/fibers/fixture_url_rewriter.php?role=sid', 'none', 8.0), true);
$window = session_inner_request('/tests/fibers/fixture_url_rewriter.php?role=trans', 'none', 8.0);
$parked = session_inner_read($parkedSock, 8.0);
$later = session_inner_request('/tests/fibers/fixture_url_rewriter.php?role=trans', 'none', 8.0);
$laterSid = json_decode(session_inner_request('/tests/fibers/fixture_url_rewriter.php?role=sid', 'none', 8.0), true);

$p = $report($parked);
$w = $report($window);
$l = $report($later);
$t->meta('parked', $parked);
$t->meta('window', $window);
$t->meta('later', $later);
$t->meta('sid_in_the_window', $windowSid);
$t->meta('sid_after', $laterSid);

// The premises: three different sessions, and the window ran while the first
// request was parked.
$t->assertTrue('the parked request started a session', ($p['id'] ?? '') !== '');
$t->assertTrue('the request in the window started another', ($w['id'] ?? '') !== '' && ($w['id'] ?? '') !== ($p['id'] ?? ''));
$t->assertTrue('and the later one a third', ($l['id'] ?? '') !== '' && ($l['id'] ?? '') !== ($w['id'] ?? ''));
$t->assertTrue('the parked request read SID as the module set it', ($p['sid_at_start'] ?? null) === 'OXRWSID=' . ($p['id'] ?? ''));

// Each response's link carries that request's own id.
$t->assertSame('the parked request\'s link carries its own id', $linkId($parked), $p['id'] ?? false);
$t->assertSame('the window\'s link carries its own id', $linkId($window), $w['id'] ?? false);
$t->assertSame('the later request\'s link carries its own id', $linkId($later), $l['id'] ?? false);

// And SID is the reader's own: its session's, or nothing.
$t->assertSame('the parked request still reads its own SID on resume', $p['sid'] ?? null, 'OXRWSID=' . ($p['id'] ?? ''));
$t->assertSame('the window reads its own SID', $w['sid'] ?? null, 'OXRWSID=' . ($w['id'] ?? ''));
$t->assertTrue(
    'a request that started no session reads no id in SID while another is parked',
    is_array($windowSid) && in_array($windowSid['sid'] ?? 'missing', [null, ''], true)
);
$t->assertTrue(
    'nor after another has ended',
    is_array($laterSid) && in_array($laterSid['sid'] ?? 'missing', [null, ''], true)
);

$t->done();
