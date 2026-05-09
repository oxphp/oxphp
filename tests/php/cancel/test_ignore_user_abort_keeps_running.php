<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$marker = '/tmp/oxphp-cancel-ignore-user-abort.marker';

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // Belt-and-suspenders: remove any stale marker from a previous run.
    @unlink($marker);

    // Opt out of the cancellation interrupt path.  Even though the client
    // disconnects ~500 ms in (curl --max-time 0.5), the script must keep
    // running until completion and write the marker file.
    ignore_user_abort(true);

    // Sleep past the client disconnect deadline.
    sleep(2);

    @file_put_contents($marker, 'finished');

    // Response is unreachable — the client is gone — but the script
    // still runs to here.
    echo "done\n";
    exit;
}

// action=check
// Wait long enough for the trigger's sleep + write to complete.
sleep(3);

$test = new TestCase('ignore_user_abort_keeps_running', 'cancel');
$exists = is_file($marker);
$content = $exists ? (file_get_contents($marker) ?: '') : '';
if ($exists) {
    unlink($marker);
}

$test->assertTrue('marker file exists despite client disconnect', $exists);
$test->assertSame('marker contents', $content, 'finished');
$test->done();
