<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The upload half of a body made in the error handler that request_parse_body()
// runs while it is still reading its options. On a multipart PUT the handler's
// own request_parse_body() writes the upload to disk and registers it, and the
// outer call's parse puts its own registry in place of that one. The end of a
// request deletes only the uploads registered at that moment, so the handler's
// file has to be kept for it from the moment its registry is replaced, or it
// stays on disk for good.
//
// Two sightings on one worker (PHP_WORKERS=1 in this profile): the PUT records
// where its upload went, and the GET after it asks whether the file is gone.

$t = new TestCase('request_parse_body_options_warning_unlinks_uploads', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

if ($sighting === 1) {
    $inner = null;
    $registeredInHandler = false;
    set_error_handler(static function () use (&$inner, &$registeredInHandler): bool {
        if ($inner === null) {
            $inner = request_parse_body();
            $registeredInHandler = is_uploaded_file((string) ($inner[1]['doc']['tmp_name'] ?? ''));
        }
        return true;
    });
    try {
        request_parse_body(['max_input_vars' => '1000Q']);
    } finally {
        restore_error_handler();
    }

    $tmp = (string) ($inner[1]['doc']['tmp_name'] ?? '');
    $t->meta('upload', $tmp);
    // The premise: the handler's call registered the upload, and the outer
    // call's registry has taken the place of the one that names it.
    $t->assertTrue('the handler\'s call registered the upload', $registeredInHandler);
    $t->assertFalse('and the outer call\'s registry replaced that one', is_uploaded_file($tmp));
    $t->assertTrue('the upload stays on disk while its request runs', is_file($tmp));

    $sharedState['request_parse_body_options_warning_upload'] = $tmp;
} else {
    $t->assertKeyExists(
        'the PUT before this request recorded its upload',
        $sharedState,
        'request_parse_body_options_warning_upload'
    );
    $tmp = (string) ($sharedState['request_parse_body_options_warning_upload'] ?? '');
    unset($sharedState['request_parse_body_options_warning_upload']);

    $t->meta('upload', $tmp);
    $t->assertFalse('the end of that request deleted the replaced upload', $tmp !== '' && file_exists($tmp));
}

$t->done();
