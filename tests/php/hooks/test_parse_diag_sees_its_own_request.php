<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request's body parse must raise its diagnostics into the request that sent
// the body.
//
// post_max_size, max_input_vars, max_file_uploads and every upload error are
// E_WARNING, and an application with set_error_handler() installed has that
// handler called for each of them. In worker mode the handler outlives the
// request that installed it — nothing here runs the shutdown that would drop it
// — so the handler an application installs at bootstrap is live for every body
// the worker ever parses. Where that parse runs therefore decides what the
// handler sees: run outside the request, it reports a limit that one client
// exceeded against the URL, method and headers of another.
//
// Two halves of the same parse, because they are raised from different places:
// the multipart phase from rfc1867_post_handler, the urlencoded one from
// php_std_post_handler. Both reach userland through the same sapi_handle_post().
//
// The arm/probe split is what makes it a worker-mode test at all. The recorder
// is installed by one request and asked about by the next, which is exactly the
// lifetime the defect lives in: the probe's body is parsed before a single line
// of the probe's own code runs, with whatever the worker had standing at that
// moment. PHP_WORKERS=1 (this profile) is what puts both on the same worker.

$phase = (string) ($_GET['phase'] ?? '');

$t = new TestCase('parse_diag_sees_its_own_request', 'hooks');

if ($phase === 'arm') {
    // Installed after TestCase's constructor on purpose — that one installs a
    // throwing handler of its own, and the last one installed is the one the
    // next request's parse will call.
    $GLOBALS['ox_parse_diag'] = [];
    set_error_handler(function (int $errno, string $errstr): bool {
        // Whether the parse is holding fiber switching down. It must: several of
        // the fields the parse works through are one slot per worker thread and
        // do not travel with a suspending request, so parking here would hand
        // the next request the pieces of this one's parse. The engine answers a
        // Fiber that tries to switch under that block with FiberError.
        $switchBlocked = false;
        try {
            (new \Fiber(static function (): void {
            }))->start();
        } catch (\FiberError) {
            $switchBlocked = true;
        }

        $GLOBALS['ox_parse_diag'][] = [
            'msg' => $errstr,
            // What a Sentry-style handler would report the request as.
            'uri' => $_SERVER['REQUEST_URI'] ?? '(no $_SERVER)',
            // Whether it was called as part of a request at all. Worker-mode
            // requests run as fibers, so outside one this is null.
            'fiber' => \Fiber::getCurrent() !== null,
            'switch_blocked' => $switchBlocked,
        ];
        return true;
    });

    $t->assertTrue('the recorder is installed for the next request', true);
    $t->done();
}

$expected = match ($phase) {
    'multipart' => 'Maximum number of allowable file uploads has been exceeded',
    'urlencoded' => 'Input variables exceeded',
    default => null,
};
$t->assertNotNull('the probe was asked for a known body shape', $expected);

$log = $GLOBALS['ox_parse_diag'] ?? null;

// Without this the rest proves nothing: a parse that raised nothing was never
// over any limit, and every check below would pass on a body PHP was happy with.
$t->assertTrue('the parse called the application error handler', is_array($log) && $log !== []);

$first = is_array($log) && $log !== [] ? $log[0] : [];

$t->assertContains(
    'the handler was told about this body\'s limit',
    (string) ($first['msg'] ?? ''),
    (string) $expected
);

// The two discriminating checks. Both are about where the parse ran, and both
// answer with the previous request — the one that armed the recorder — when it
// ran on the worker's own stack instead of inside this request.
$t->assertSame(
    'the handler saw THIS request in $_SERVER',
    $first['uri'] ?? null,
    $_SERVER['REQUEST_URI']
);
$t->assertTrue('the handler ran inside a request fiber', ($first['fiber'] ?? null) === true);

// And that fiber must not be able to leave: the parse holds thread-wide SAPI
// state that a suspending request does not take with it.
$t->assertTrue(
    'and the parse held fiber switching down while it ran',
    ($first['switch_blocked'] ?? null) === true
);

// And the parse itself still did its job, up to the limit it stopped at. The
// urlencoded count is 1001 rather than 1000 because php-src registers the
// variable and then counts it: the one that trips the limit is already in.
if ($phase === 'multipart') {
    $t->assertSame('$_FILES holds the uploads accepted before the limit', count($_FILES), 20);
} else {
    $t->assertSame('$_POST holds the variables accepted before the limit', count($_POST), 1001);
}

$t->done();
