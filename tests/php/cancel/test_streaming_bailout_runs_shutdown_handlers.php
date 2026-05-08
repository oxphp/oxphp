<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$marker = '/tmp/oxphp-cancel-streaming-bailout.marker';

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // Belt-and-suspenders: remove any stale marker from a previous run.
    @unlink($marker);

    register_shutdown_function(function () use ($marker) {
        @file_put_contents($marker, 'shutdown_ran');
    });

    header('Content-Type: text/event-stream');
    echo "data: prefill\n\n";
    oxphp_stream_flush();

    // Block long enough for curl --max-time to disconnect the client
    // and for the interrupt / bailout path to fire.  3 s is well within
    // the runner's 15 s http_request timeout, and the check action waits
    // 4 s before reading the marker, giving the shutdown function time to run.
    sleep(3);

    echo "data: never_reached\n\n";
    oxphp_stream_flush();
    exit;
}

// action=check
// Wait for the trigger action's shutdown function to write the marker file.
// The trigger PHP script sleeps 3 s, and this request arrives ~0.7 s into
// the trigger's execution, so waiting 4 s gives the bailout path ~1.3 s of
// slack after the trigger's sleep completes.
sleep(4);

$test = new TestCase('streaming_bailout_runs_shutdown_handlers', 'cancel');
$exists = is_file($marker);
$content = $exists ? (file_get_contents($marker) ?: '') : '';
// Do not use @unlink(): PHP 8 custom error handlers still fire despite @.
if ($exists) {
    unlink($marker);
}

$test->assertTrue('marker file exists after disconnect', $exists);
$test->assertSame('marker contents', $content, 'shutdown_ran');
$test->done();
