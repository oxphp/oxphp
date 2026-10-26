<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() that stops before replacing anything. With a
// post_max_size the multipart body is over, the parse throws before it
// registers uploads of its own, so the request's uploads stay the current ones:
// still registered, and released once — by the end of the request, as any
// current upload is. A later call in the same request that does
// replace them makes them superseded, and they must still be released once.
//
// Three sightings on one worker (PHP_WORKERS=1 in this profile):
//   1. POST — one call that stops early; the uploads stay current;
//   2. POST — the same call, then one that replaces the uploads; and the file
//      sighting 1 left is gone;
//   3. GET  — the file sighting 2 left is gone.
// $sharedState carries the paths, which also says the worker survived the end
// of each request: a replacement worker starts with it empty.

$t = new TestCase('request_parse_body_early_exit_keeps_uploads', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$key = 'request_parse_body_early_exit_upload';

if ($sighting > 1) {
    $t->assertKeyExists('the request before this one recorded its upload', $sharedState, $key);
    $previous = (string) ($sharedState[$key] ?? '');
    unset($sharedState[$key]);
    $t->meta('previous_upload', $previous);
    $t->assertFalse('the end of that request deleted its upload', $previous !== '' && file_exists($previous));
}

if ($sighting < 3) {
    $tmp = (string) ($_FILES['doc']['tmp_name'] ?? '');
    $t->assertTrue('the request\'s own parse registered the upload', is_uploaded_file($tmp));

    $threw = '';
    try {
        request_parse_body(['post_max_size' => 1]);
    } catch (\RequestParseBodyException $e) {
        $threw = $e->getMessage();
    }
    $t->assertContains('a parse over post_max_size stops early', $threw, 'exceeds the limit of 1 bytes');
    $t->assertTrue('and the upload stays registered', is_uploaded_file($tmp));

    if ($sighting === 2) {
        $parsed = request_parse_body();
        $t->assertSame('a parse that runs to the end finds the body already read', $parsed, [[], []]);
        $t->assertFalse('and replaces the registered uploads', is_uploaded_file($tmp));
        $t->assertTrue('the upload stays on disk while its request runs', is_file($tmp));
    }

    $sharedState[$key] = $tmp;
}

$t->done();
