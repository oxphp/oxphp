<?php

declare(strict_types=1);

// Driven over HTTP by the locale tests; answers one JSON object.
//
//   set=LC_<NAME>  first call setlocale() for that category with 'C', and
//                  report what it returned
//
// escapeshellarg() drops a byte that is not valid in LC_CTYPE's encoding.
// musl's C locale takes every byte as valid, so on the Alpine image `escaped`
// shows whether libc used C or C.UTF-8; glibc's C drops the byte as well.
header('Content-Type: application/json');

$out = ['worker' => oxphp_worker_id()];
if (isset($_GET['set'])) {
    $out['set'] = setlocale(constant((string) $_GET['set']), 'C');
}
$out['ctype'] = setlocale(LC_CTYPE, '0');
$out['escaped'] = bin2hex(escapeshellarg("a\xffb"));

echo json_encode($out);
