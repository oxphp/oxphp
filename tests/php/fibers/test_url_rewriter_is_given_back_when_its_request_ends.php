<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// What a request gave PHP's URL rewriter, and the SID constant, are given back
// when the request ends, so the next request on the worker starts without them.
//
// Left behind, the rewriter still counts as running for the next request, which
// then gets no rewriting of its own — its links go out without its session id —
// and SID names the session of the request before it.
//
// The next request has to start straight after the first one ends. Any other
// request resuming in between puts its own state back and so clears the
// thread, which is what this test's own request does whenever the response it
// waits for arrives. So the first request keeps its session open while it
// parks, and the worker takes no other request until that one has ended. Two
// queue behind it: one that only reads SID, then one that starts a session of
// its own. This request waits for theirs, not for the first one's, so it cannot
// resume before they are taken.

$t = new TestCase('url_rewriter_is_given_back_when_its_request_ends', 'fibers');

/** The value the link in $body was rewritten with, or null. */
$linkId = static function (string $body): ?string {
    return preg_match('~href="/next\.php\?OXRWSID=([^"&]+)"~', $body, $m) ? $m[1] : null;
};
/** What the request reported in its <p>. */
$report = static function (string $body): array {
    return preg_match('~<p>(.*)</p>~', $body, $m) ? (json_decode($m[1], true) ?: []) : [];
};

unset($sharedState['url_rewriter_parked']);
$firstSock = session_inner_send('/tests/fibers/fixture_url_rewriter.php?role=trans&hold=1&park=0.6', 'none', 8.0);
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['url_rewriter_parked']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$sidSock = session_inner_send('/tests/fibers/fixture_url_rewriter.php?role=sid', 'none', 8.0);
// So the two arrive in this order.
oxphp_usleep(50_000);
$transSock = session_inner_send('/tests/fibers/fixture_url_rewriter.php?role=trans', 'none', 8.0);

$sidAfter = json_decode(session_inner_read($sidSock, 8.0), true);
$next = session_inner_read($transSock, 8.0);
$first = session_inner_read($firstSock, 8.0);

$f = $report($first);
$n = $report($next);
$t->meta('first', $first);
$t->meta('sid_after', $sidAfter);
$t->meta('next', $next);

// The premises: the first request parked with its session open, and both
// others ran only after it had ended.
$t->assertTrue('the first request parked', isset($sharedState['url_rewriter_parked']));
$t->assertTrue('with its session open', ($f['held'] ?? false) === true);
$t->assertTrue('and started a session', ($f['id'] ?? '') !== '');
$t->assertTrue('the one reading SID ran after it ended', is_array($sidAfter) && ($sidAfter['at'] ?? 0) > ($f['ended_at'] ?? INF));
$t->assertTrue('and so did the next session', ($n['ended_at'] ?? 0) > ($f['ended_at'] ?? INF) && ($n['id'] ?? '') !== '' && ($n['id'] ?? '') !== ($f['id'] ?? ''));
$t->assertSame('the first request\'s link carries its own id', $linkId($first), $f['id'] ?? false);

$t->assertTrue(
    'a request taken straight after it reads no id in SID',
    is_array($sidAfter) && in_array($sidAfter['sid'] ?? 'missing', [null, ''], true)
);
$t->assertSame('and the next session\'s link carries its own id', $linkId($next), $n['id'] ?? false);

$t->done();
