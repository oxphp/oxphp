<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Behavioural contract: a max_execution_time timeout (SIGALRM-driven
// in traditional mode) flows through the unified Zend interrupt
// handler, which calls oxphp_metrics_cancelled(2) → the
// `oxphp_request_cancelled_total{reason="timeout"}` counter
// increments. The trigger phase self-cancels; the check phase
// asserts the counter incremented and the other reason labels are
// also exposed (gauge surface contract).

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // 1 s budget; the sleep is 3 s — Zend's SIGALRM fires at 1 s,
    // EG(timed_out) becomes true, the next opcode boundary calls
    // oxphp_zend_interrupt_handler, the unified bailout records the
    // reason and bumps the counter, then zend_error_noreturn fires.
    set_time_limit(1);
    sleep(3);
    echo "never reached";
    return;
}

// action=check
// Give the trigger's bailout a moment to flush the counter store.
sleep(1);

$metrics = @file_get_contents('http://127.0.0.1:9090/metrics');

$test = new TestCase('cancellation_counters', 'observability');
$test->assertTrue('metrics endpoint reachable', is_string($metrics));

if (!is_string($metrics)) {
    $test->done();
    return;
}

$counter = static function (string $body, string $reason): int {
    $pattern = '/^oxphp_request_cancelled_total\{reason="'
        . preg_quote($reason, '/') . '"\} (\d+)$/m';
    return preg_match($pattern, $body, $m) ? (int)$m[1] : -1;
};

$timeout_count = $counter($metrics, 'timeout');
$client_abort_count = $counter($metrics, 'client_abort');
$shutdown_count = $counter($metrics, 'shutdown');

$test->assertTrue(
    'timeout counter line is present',
    $timeout_count >= 0
);
$test->assertTrue(
    'client_abort counter line is present',
    $client_abort_count >= 0
);
$test->assertTrue(
    'shutdown counter line is present',
    $shutdown_count >= 0
);
$test->assertTrue(
    'timeout counter incremented at least once (was ' . $timeout_count . ')',
    $timeout_count >= 1
);

$test->done();
