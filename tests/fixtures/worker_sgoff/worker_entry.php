<?php

declare(strict_types=1);

// Worker entry for the superglobals_off_worker profile.
//
// It exists separately from tests/fixtures/worker/worker_entry.php because
// that one routes on $_SERVER['REQUEST_URI'], and this profile runs with
// SUPERGLOBALS_ENABLED=false, where a request's $_SERVER holds only the four
// keys PHP registers itself — REQUEST_TIME, REQUEST_TIME_FLOAT, argc, argv —
// so the dispatcher would find no URI for any request and never reach a test
// file. (This bootstrap section is the exception: it runs before any request
// and sees the boot $_SERVER, REQUEST_URI = '/' included, which is not a
// route.) Routing through the object API is what a worker has to do under
// that setting, so the entry that covers the setting is written that way.

oxphp_worker(function (): void {
    $path = oxphp_http_request()->path();

    if (preg_match('#^/tests/.+\.php$#', $path)) {
        $testFile = '/var/www/html/public' . $path;
        if (file_exists($testFile)) {
            include $testFile;
            return;
        }
    }

    header('Content-Type: application/json');
    echo json_encode(['error' => 'no such test', 'path' => $path], JSON_UNESCAPED_SLASHES);
});
