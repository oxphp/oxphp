<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_all_reject_member', 'async');

// await_all must reject as soon as a member throws (Promise.all fast-reject),
// surfacing that member's error — AND cancel the remaining members rather than
// leaving them running unobserved (the reject branch of the bail-out strand,
// the same straggler handling as the timeout branch).
$marker = sys_get_temp_dir() . '/oxphp_allreject_' . getmypid() . '_' . uniqid('', true);

// Rejects after 50ms — long enough for the trailing CPU-bound member to already
// be running on another worker when the rejection lands (ASYNC_WORKERS=4).
$bad = oxphp_async(function (): never {
    usleep(50_000);
    throw new \RuntimeException('boom');
});
$cpu = oxphp_async(function () use ($marker): int {
    try {
        $x = 0;
        while (true) {            // never yields; JMP backedge checks vm_interrupt
            $x++;
        }
    } finally {
        file_put_contents($marker, 'cancelled');
    }
    return 0; // unreachable
});

$threw = false;
$msg = '';
try {
    oxphp_async_await_all([$bad, $cpu], 5.0);
} catch (\OxPHP\Async\AsyncException $e) {
    $threw = true;
    $msg = $e->getMessage();
}

$t->assertTrue('await_all rejected when a member threw', $threw);
$t->assertContains('reject surfaces the member exception', $msg, 'boom');

// The trailing CPU-bound member must be interrupted (Path B) so its finally
// runs — proving the reject path strands the remaining members, not just the
// timeout path.
$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}
$t->assertTrue('remaining CPU-bound member cancelled on reject (finally ran)', file_exists($marker));

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
