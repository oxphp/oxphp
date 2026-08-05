<?php

declare(strict_types=1);

// Inner self-request for hooks/test_files_survive_suspend. It is served only
// while the outer request's fiber is parked in a hooked sleep(), which is the
// window that test needs: uploading a file here writes this request's own
// temp file and registers it in SG(rfc1867_uploaded_files) — the thread-wide
// slot the parked request comes back to.
//
// Echoes what it received, so the outer test can assert on two things at once:
// that this request ran inside the window at all, and that its own upload
// worked, which is what makes the state it leaves behind real.
//
// Must not suspend: no await, no sleep, no socket read. A suspend here would
// park this fiber too, and the outer request could resume before this one had
// touched the uploaded-file state at all — which empties the test out silently.

$file = $_FILES['doc'] ?? null;

if (!is_array($file)
    || ($file['error'] ?? -1) !== UPLOAD_ERR_OK
    || !is_uploaded_file((string) ($file['tmp_name'] ?? ''))) {
    http_response_code(500);
    echo 'INNER-FAIL: intruder upload did not arrive: ' . var_export($file, true) . "\n";
    return;
}

echo 'INNER-OK ' . $file['name'] . ' ' . file_get_contents((string) $file['tmp_name']) . "\n";
