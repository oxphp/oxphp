<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The multipart half of request_parse_body() parsing a POST body a second time.
// The first parse registered the request's uploads; the second registers its
// own (none — the body is already read) in their place without deleting the
// files the first one wrote. The end of a request deletes only the uploads
// registered at that moment, so the first parse's files have to be released by
// the end of the request as well, or they stay on disk for good.
//
// Two sightings on one worker (PHP_WORKERS=1 in this profile): the POST records
// where its upload went, and the GET after it asks whether the file is gone.
// While the POST is still running the file must stay: it is the request's own,
// as it is under php-fpm.

$t = new TestCase('request_parse_body_unlinks_superseded_uploads', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

if ($sighting === 1) {
    $tmp = (string) ($_FILES['doc']['tmp_name'] ?? '');
    $t->assertTrue('the request\'s own parse registered the upload', is_uploaded_file($tmp));

    $parsed = request_parse_body();

    $t->assertSame('the second parse finds the body already read', $parsed, [[], []]);
    // The premise: the registry the end of the request reads no longer names
    // this file, so it is the replaced registry that has to release it.
    $t->assertFalse('and its own registry replaced the first one', is_uploaded_file($tmp));
    $t->assertTrue('the upload stays on disk while its request runs', is_file($tmp));

    $sharedState['request_parse_body_superseded_upload'] = $tmp;
} else {
    $t->assertKeyExists(
        'the POST before this request recorded its upload',
        $sharedState,
        'request_parse_body_superseded_upload'
    );
    $tmp = (string) ($sharedState['request_parse_body_superseded_upload'] ?? '');
    unset($sharedState['request_parse_body_superseded_upload']);

    $t->meta('upload', $tmp);
    $t->assertFalse('the end of that request deleted the replaced upload', $tmp !== '' && file_exists($tmp));
}

$t->done();
