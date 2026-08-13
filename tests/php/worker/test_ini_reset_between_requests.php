<?php
// Two halves of one guarantee, one per phase.
//
// A worker serves every request it ever sees inside a single request startup, so
// nothing between two requests restores the ini directives one of them changed:
// `ini_set()`, `set_time_limit()` and `error_reporting()` write into
// `EG(modified_ini_directives)` and the engine only unwinds that at a real
// request shutdown. Left alone, a request that turns `display_errors` on turns it
// on for every later request on that worker, and `set_time_limit()` becomes the
// deadline of requests that never asked for one. Every other SAPI ends those with
// the request.
//
// `set`: the request sees the values its worker booted with, changes four of
// them, and confirms the changes took.
//
// `check`: the next request must see the boot values again — not the ones the
// `set` request left (that is the leak), and not the engine defaults either (that
// would mean the rollback also discarded the worker's own bootstrap
// configuration, which is application config, not request state). `user_agent`
// carries that second half: the boot script sets it to a value the engine would
// never produce on its own, so "restored too far" and "restored correctly" are
// distinguishable.
//
// Both phases must stay adjacent and land on one worker, which is why this is
// listed only in a profile that pins PHP_WORKERS=1. That precondition is
// checked rather than assumed: served by two different workers, both phases
// would pass on any build at all — `set` would see the boot values because
// nothing had changed them on its worker, and `check` would see them because
// its worker never ran `set` — so the guarantee would go untested and the
// suite would stay green saying so. Each phase records the worker that served
// it and `check` fails on a mismatch, naming the setting that has to change.
//
// Written without test_helper.php: the tests under tests/php/worker pull it in
// with a bare `require`, so a worker serving two of them fatals on the class
// redeclare.

// From the worker boot scope (tests/fixtures/worker/worker_entry.php), captured
// there rather than hardcoded here: the baseline is whatever that worker booted
// with, php.ini included.
if (!isset($iniBoot) || !is_array($iniBoot)) {
    http_response_code(500);
    echo "FAIL: \$iniBoot is not in scope — this test needs the worker entry file"
        . " at tests/fixtures/worker/worker_entry.php\n";
    return;
}

$fail = [];
$phase = $_GET['phase'] ?? 'set';

// The two phases talk through a file rather than worker-scope state, because
// the question it answers — did the same worker serve both? — is exactly the
// one worker-scope state cannot answer.
$ledger = sys_get_temp_dir() . '/oxphp_worker_ini_reset_worker_id';
$workerId = oxphp_worker_id();

// What the `set` phase leaves behind, and therefore what the `check` phase must
// NOT see. Derived from the boot values so every one of them is a real change.
$probe = [
    'user_agent'         => 'oxphp-req-probe',
    'precision'          => $iniBoot['precision'] === '7' ? '9' : '7',
    'display_errors'     => $iniBoot['display_errors'] === '0' ? '1' : '0',
    // Large enough that the timer it arms cannot fire between the two requests.
    'max_execution_time' => '1234',
];

if ($phase === 'set') {
    foreach ($iniBoot as $name => $bootValue) {
        $seen = ini_get($name);
        if ($seen !== $bootValue) {
            $fail[] = "on entry $name = " . var_export($seen, true) . ', expected the'
                . ' boot value ' . var_export($bootValue, true)
                . ' — something before this request left it changed';
        }
    }

    // set_time_limit() rather than ini_set(): it is the call applications reach
    // for, and it writes the same ini entry.
    set_time_limit((int) $probe['max_execution_time']);
    ini_set('user_agent', $probe['user_agent']);
    ini_set('precision', $probe['precision']);
    ini_set('display_errors', $probe['display_errors']);

    foreach ($probe as $name => $want) {
        $seen = ini_get($name);
        if ($seen !== $want) {
            $fail[] = "after setting it, $name = " . var_export($seen, true)
                . ', expected ' . var_export($want, true)
                . ' — the change this phase depends on did not take';
        }
    }

    file_put_contents($ledger, (string) $workerId);
} else {
    // Guarded rather than suppressed: an unreadable ledger means the phases ran
    // out of order or on different servers, which is a failure of this test's
    // own setup and has to say so rather than fail as a leak.
    $setWorker = is_file($ledger) ? (string) file_get_contents($ledger) : '';
    if ($setWorker !== '') {
        unlink($ledger);
    }

    if ($setWorker === '') {
        $fail[] = 'the set phase left no record of the worker that served it —'
            . ' the two phases must run in order and against the same server';
    } elseif ($setWorker !== (string) $workerId) {
        $fail[] = "the set phase ran on worker $setWorker and this one on worker"
            . " $workerId, so nothing here was ever tested: both phases pass on any"
            . ' build when they land on different workers. This test needs a profile'
            . ' with PHP_WORKERS=1';
    }

    foreach ($iniBoot as $name => $bootValue) {
        $seen = ini_get($name);
        if ($seen === $bootValue) {
            continue;
        }
        if ($seen === $probe[$name]) {
            $fail[] = "$name = " . var_export($seen, true) . ' — the value the previous'
                . ' request on this worker set; a request\'s ini change outlived its'
                . ' request instead of ending with it';
        } else {
            $fail[] = "$name = " . var_export($seen, true) . ', expected the boot value '
                . var_export($bootValue, true) . ' (the previous request set '
                . var_export($probe[$name], true) . ') — the rollback went past what'
                . ' the worker booted with and discarded its bootstrap configuration';
        }
    }
}

if ($fail !== []) {
    http_response_code(500);
    echo 'FAIL: ' . implode('; ', $fail) . "\n";
    return;
}

echo "OK phase=$phase\n";
