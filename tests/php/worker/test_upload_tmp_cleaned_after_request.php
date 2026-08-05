<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Two halves of one guarantee, one per phase.
//
// `upload`: the request is given its uploaded file at all — in worker mode
// nothing used to parse the body, so $_FILES was empty for every multipart POST
// ever served.
//
// `check`: the temp file that upload left behind is gone by the next request.
// Every other SAPI unlinks it when the request ends; a worker's request has no
// such end of its own, so without an explicit per-request cleanup the files pile
// up in upload_tmp_dir for the life of the worker. Asserting the file is gone
// rather than "the upload worked" is the point: the leak is invisible to the
// request that causes it.
//
// The two phases talk through a file rather than worker-scope state so the check
// holds under any pool size — the temp file lives on the container filesystem,
// whichever worker answers.

$t = new TestCase('upload_tmp_cleaned_after_request', 'worker');

$ledger = sys_get_temp_dir() . '/oxphp_worker_upload_tmp_ledger';

if (($_GET['phase'] ?? 'upload') === 'upload') {
    $t->assertNotEmpty('$_FILES is populated for a multipart POST', $_FILES);

    $file = $_FILES ? reset($_FILES) : [];
    $tmp = is_array($file) ? (string) ($file['tmp_name'] ?? '') : '';

    $t->assertSame('upload error is UPLOAD_ERR_OK', is_array($file) ? $file['error'] ?? null : null, UPLOAD_ERR_OK);
    $t->assertTrue('the upload has a temp file on disk', $tmp !== '' && is_file($tmp));

    file_put_contents($ledger, $tmp);
} else {
    // Guarded rather than suppressed: TestCase turns every warning into an
    // ErrorException, and `@` does not stop a custom error handler from being
    // called, so unlinking a path that is not there would fatal the test.
    $tmp = '';
    if (is_file($ledger)) {
        $tmp = (string) file_get_contents($ledger);
        unlink($ledger);
    }

    $t->assertNotEmpty('the upload phase recorded a temp path', $tmp);
    $t->assertFalse(
        "the previous request's upload temp file is gone ({$tmp})",
        $tmp !== '' && file_exists($tmp)
    );
}

$t->done();
