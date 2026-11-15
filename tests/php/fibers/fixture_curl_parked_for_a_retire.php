<?php

declare(strict_types=1);

// The request fibers/test_curl_transfer_ended_by_a_retire_leaves_its_handle_usable
// sends: a curl_exec() to a listener that accepts and never answers, parked there
// until retiring the worker kills it. Not a TestCase: what it learns goes to the
// file its probe reads, since this worker is gone by then.
//
// With ?throw=1 the trigger answers with the start of a body it never finishes, and
// the write callback throws on it. ext/curl lets the transfer go on with the
// exception pending (interface.c curl_write(), 8.4 and 8.5), and the engine runs
// no PHP callback while it is: zend_call_function() returns at once. A progress
// callback that runs once the kill has ended the wait is therefore one that found
// the exception gone.

require_once __DIR__ . '/bailout_leak_probe.php';

$port = (int) ($_GET['port'] ?? 0);
$h = curl_init("http://127.0.0.1:{$port}/");
curl_setopt_array($h, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
if (isset($_GET['throw'])) {
    $threw = false;
    curl_setopt_array($h, [
        CURLOPT_WRITEFUNCTION => static function ($h, string $data) use (&$threw, &$sharedState): int {
            $threw = true;
            $sharedState['curl_retire_threw'] = true;
            throw new \RuntimeException('the write callback threw');
        },
        CURLOPT_NOPROGRESS => false,
        CURLOPT_XFERINFOFUNCTION => static function () use (&$threw): int {
            if ($threw) {
                OxphpBailoutLeak::note('curl_retire', 'callback_after_the_throw', true);
            }
            return 0;
        },
    ]);
}

// Runs as the killed request ends. Neither cURL call in it waits, so the kill does
// not cut it short: curl_errno() reads what curl_exec() left behind, and libcurl
// refuses to add the handle to a multi handle (CURLM_ADDED_ALREADY) for as long as
// it still has it in the transfer curl_exec() began.
register_shutdown_function(static function () use ($h): void {
    $errno = curl_errno($h);
    $mh = curl_multi_init();
    $added = curl_multi_add_handle($mh, $h);
    if ($added === CURLM_OK) {
        curl_multi_remove_handle($mh, $h);
    }
    OxphpBailoutLeak::note('curl_retire', 'after_the_kill', ['errno' => $errno, 'added' => $added]);
});

$sharedState['curl_retire_parked'] = true;
curl_exec($h);
OxphpBailoutLeak::note('curl_retire', 'returned', true);
echo 'not reached';
