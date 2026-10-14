<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A variable a request adds with output_add_rewrite_var() is appended to the
// links of that request's response, and of no other.
//
// The rewriter keeps the variables it appends, and whether its output handler
// is already running, in state that is one per thread rather than one per
// request. Left on the thread, a variable a request added in the window is
// appended to a parked request's links as well, once that request resumes and
// ends, and the request that added it gets no rewriting at all, because the
// handler counts as running already. Left after the request, the same holds
// for every later request on the worker.
//
// A request adds tok=parked, prints a link and parks. Another adds tok=window
// and ends. The first resumes and ends. After both, a third adds tok=later.

$t = new TestCase('url_rewrite_var_stays_with_its_request', 'fibers');

/** Every tok= value the link in $body was rewritten with. */
$toks = static function (string $body): array {
    if (!preg_match('~href="/next\.php\?([^"]*)"~', $body, $m)) {
        return [];
    }
    preg_match_all('~(?:^|&(?:amp;)?)tok=([^&]*)~', $m[1], $all);

    return $all[1];
};

unset($sharedState['url_rewriter_parked']);
$parkedSock = session_inner_send('/tests/fibers/fixture_url_rewriter.php?role=var&tok=parked&park=0.6', 'none', 8.0);
// Waiting parks this request, which lets the worker take the first one.
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['url_rewriter_parked']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$window = session_inner_request('/tests/fibers/fixture_url_rewriter.php?role=var&tok=window', 'none', 8.0);
$parked = session_inner_read($parkedSock, 8.0);
$later = session_inner_request('/tests/fibers/fixture_url_rewriter.php?role=var&tok=later', 'none', 8.0);

$t->meta('parked', $parked);
$t->meta('window', $window);
$t->meta('later', $later);

$t->assertTrue('the first request parked', isset($sharedState['url_rewriter_parked']));
$t->assertSame('the parked request\'s link carries its own variable and no other', $toks($parked), ['parked']);
$t->assertSame('the window\'s link carries its own', $toks($window), ['window']);
$t->assertSame('the later request\'s link carries its own', $toks($later), ['later']);

$t->done();
