<?php
// What ini_get() says about OPcache has to be what OPcache is doing.
//
// opcache.enable is settable per request, and what a request can do with it is
// only turn it off: the handler refuses to switch the accelerator back on
// mid-flight and instead drops both the directive's value and the accelerator's
// own live flag. Every other SAPI raises that flag again at the start of the
// next request, in OPcache's RINIT — which a worker runs once, when it boots,
// and never again. So on a worker the accelerator stays down for the rest of
// the worker's life whatever happens to the directive afterwards.
//
// That is the constraint. What this test pins is the consequence: since the
// rollback between requests can put the value back but cannot put the
// accelerator back, restoring the value alone would leave ini_get() answering
// "enabled" for a worker that compiles every file from source — the one
// failure that costs debugging time, because the reading that is easiest to
// take is the one that is wrong.
//
// The assertion is agreement rather than a fixed value, on purpose: a worker
// that some day does bring the accelerator back is also correct, and this test
// keeps passing for it. What must never happen is the two disagreeing.
//
// Both phases must land on one worker, which is why this is listed only in a
// profile that pins PHP_WORKERS=1, and the same-worker precondition is checked
// rather than assumed — served by two workers, the second phase would read an
// untouched worker and pass on any build at all.
//
// Written without test_helper.php: the tests under tests/php/worker pull it in
// with a bare `require`, so a worker serving two of them fatals on the class
// redeclare.

$fail = [];
$phase = $_GET['phase'] ?? 'off';

// The two phases talk through a file rather than worker-scope state, because
// the question it answers — did the same worker serve both? — is exactly the
// one worker-scope state cannot answer.
$ledger = sys_get_temp_dir() . '/oxphp_worker_opcache_enable_worker_id';
$workerId = oxphp_worker_id();

if (!function_exists('opcache_get_status')) {
    http_response_code(500);
    echo "FAIL: OPcache is not loaded, so this profile cannot run this test\n";
    return;
}

// opcache_get_status() reports the accelerator's own live flag, which is the
// half that ini_get() cannot see.
$status = opcache_get_status(false);
$live = is_array($status) ? ($status['opcache_enabled'] ?? null) : false;

if ($phase === 'off') {
    if ($live !== true) {
        $fail[] = 'OPcache is not running on entry (opcache_get_status() reports '
            . var_export($live, true) . ') — this test needs it enabled to have'
            . ' anything to turn off';
    } elseif (($seen = ini_get('opcache.enable')) !== '1') {
        $fail[] = "on entry opcache.enable = " . var_export($seen, true)
            . ", expected '1' — something before this request left it changed";
    } else {
        ini_set('opcache.enable', '0');

        $after = opcache_get_status(false);
        $liveAfter = is_array($after) ? ($after['opcache_enabled'] ?? null) : false;
        if ($liveAfter !== false) {
            $fail[] = 'after ini_set(\'opcache.enable\', \'0\') the accelerator is'
                . ' still running (opcache_get_status() reports '
                . var_export($liveAfter, true) . ') — the change this phase'
                . ' depends on did not take';
        }
        if (($seen = ini_get('opcache.enable')) !== '0') {
            $fail[] = 'after turning it off, opcache.enable = '
                . var_export($seen, true) . ", expected '0'";
        }
    }

    file_put_contents($ledger, (string) $workerId);
} else {
    // Guarded rather than suppressed: an unreadable ledger means the phases ran
    // out of order or on different servers, which is a failure of this test's
    // own setup and has to say so rather than fail as a disagreement.
    $setWorker = is_file($ledger) ? (string) file_get_contents($ledger) : '';
    if ($setWorker !== '') {
        unlink($ledger);
    }

    if ($setWorker === '') {
        $fail[] = 'the off phase left no record of the worker that served it —'
            . ' the two phases must run in order and against the same server';
    } elseif ($setWorker !== (string) $workerId) {
        $fail[] = "the off phase ran on worker $setWorker and this one on worker"
            . " $workerId, so nothing here was ever tested: both phases pass on"
            . ' any build when they land on different workers. This test needs a'
            . ' profile with PHP_WORKERS=1';
    }

    $reported = ini_get('opcache.enable');
    $expected = $live ? '1' : '0';
    if ($reported !== $expected) {
        $fail[] = "opcache.enable reads $reported while the accelerator is "
            . ($live ? 'running' : 'down')
            . ' — an application asking whether its code is being cached is told'
            . ' the opposite of what OPcache is doing';
    }
}

if ($fail !== []) {
    http_response_code(500);
    echo 'FAIL: ' . implode('; ', $fail) . "\n";
    return;
}

echo "OK phase=$phase\n";
