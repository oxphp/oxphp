<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$marker = '/tmp/oxphp-cancel-ignore-user-abort.marker';
// Written after the script has written to its gone client, so it tells a script
// that kept running until its next write apart from one that kept running.
$pastWrite = '/tmp/oxphp-cancel-ignore-user-abort.past-write';

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // Belt-and-suspenders: remove any stale marker from a previous run.
    @unlink($marker);
    @unlink($pastWrite);

    // Opt out of being ended when the client leaves.  Even though the client
    // disconnects ~500 ms in (curl --max-time 0.5), the script must keep
    // running until completion and write both marker files.
    ignore_user_abort(true);

    // Sleep past the client disconnect deadline.
    sleep(2);

    @file_put_contents($marker, 'finished');

    // Response is unreachable — the client is gone — but the script
    // still runs to here, and on past the write: flush() hands the echo to the
    // server now rather than at the end of the request, which is the point at
    // which a script whose client has left finds out.
    echo "done\n";
    flush();

    // What the script was told by then: a run past the write proves nothing if
    // the server had not yet noticed the client was gone.
    @file_put_contents($pastWrite, (string) connection_aborted());
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

$pastWriteExists = is_file($pastWrite);
$abortedAtWrite = $pastWriteExists ? (file_get_contents($pastWrite) ?: '') : '';
if ($pastWriteExists) {
    unlink($pastWrite);
}

$test->assertTrue('marker file exists despite client disconnect', $exists);
$test->assertSame('marker contents', $content, 'finished');
$test->assertTrue('and the script ran on past a write to the client it had lost', $pastWriteExists);
$test->assertSame('and knew by then that the client was gone', $abortedAtWrite, '1');
$test->done();
