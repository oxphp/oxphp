<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request that runs past memory_limit and then waits — in a sleep, a read, an
// await — is the request the error ends, and a request that runs on the same
// worker while it would have been waiting is not touched.
//
// From PHP 8.4.27 / 8.5.12 the engine lets such an allocation through and
// reports the error at the next point that checks for it, which for a wait is
// only once the wait has returned. In between, the worker runs its other
// requests, and the first of them to reach a check had the error reported in
// it: one that the worker was starting died before its handler ran and never
// answered, any other was ended with the error the waiting request ran into,
// and the waiting request answered as if nothing had happened.
//
// ?park= names the wait: sleep, read, await or any. ?park=task runs the same
// case in two oxphp_async() tasks, which this profile runs on one thread.

$park = (string) ($_GET['park'] ?? 'sleep');
$t = new TestCase('oom_before_a_park_stays_with_its_request', 'fibers');
$t->meta('park', $park);

$send = static function (string $query) use ($t) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$query: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/fibers/fixture_oom_before_a_park.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    return $sock;
};

$read = static function ($sock, string $label) use ($t): string {
    if ($sock === null) {
        return '';
    }
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$label response", $resp);

    return $resp;
};

if ($park === 'task') {
    $oom = oxphp_async(static function (): string {
        set_error_handler(null);
        ini_set('display_errors', '0');
        ini_set('log_errors', '0');
        ini_set('memory_limit', '64M');
        $s = str_repeat('x', 24 << 20);
        $doubled = $s . $s;
        sleep(3);

        // Read after the wait: a local no one reads is an assignment OPcache's
        // optimizer drops, and the allocation with it.
        return 'WOKE: ' . strlen($doubled);
    });
    $bystander = oxphp_async(static function (): string {
        sleep(1);

        return 'BYSTANDER-RAN: 1000';
    });

    $started = microtime(true);
    $oomResult = null;
    try {
        $oomResult = oxphp_async_await($oom, 10.0);
    } catch (\Throwable $e) {
        $oomResult = get_class($e) . ': ' . $e->getMessage();
    }
    $elapsed = microtime(true) - $started;
    $t->meta('task that ran out', $oomResult);
    $t->meta('it answered after (s)', round($elapsed, 3));

    $bystanderResult = null;
    try {
        $bystanderResult = oxphp_async_await($bystander, 5.0);
    } catch (\Throwable $e) {
        $bystanderResult = get_class($e) . ': ' . $e->getMessage();
    }
    $t->meta('bystander task', $bystanderResult);

    // A task's ini changes stay on its thread.
    oxphp_async_await(oxphp_async(static function (): void {
        ini_restore('memory_limit');
        ini_restore('display_errors');
        ini_restore('log_errors');
    }), 5.0);

    $t->assertContains('the task that ran out ends in the error', (string) $oomResult, 'Allowed memory size of 67108864');
    // Reported before the wait, not after it: a wait that ran to its end would
    // have taken three seconds.
    $t->assertNotContains('without its wait running to the end', (string) $oomResult, 'WOKE');
    $t->assertLessThan('and promptly', $elapsed, 1.5);
    $t->assertSame('the task beside it runs to its end', $bystanderResult, 'BYSTANDER-RAN: 1000');
    $t->done();
}

$started = microtime(true);
$oomSock = $send("step=oom&park=$park");
$bystanderSock = $send('step=bystander');

$oom = $read($oomSock, 'oom');
$elapsed = microtime(true) - $started;
$t->meta('it answered after (s)', round($elapsed, 3));
$bystander = $read($bystanderSock, 'bystander');
$probe = $read($send('step=probe'), 'probe');

// Reported in the request that ran out, before its wait: a wait that ran to its
// end would have taken three seconds, and printed WOKE.
$t->assertContains('the request that ran out shows the error', $oom, 'Allowed memory size of 67108864');
$t->assertNotContains('without its wait running to the end', $oom, 'WOKE');
$t->assertLessThan('and promptly', $elapsed, 1.5);

// Not in the request beside it, which runs to its end.
$t->assertContains('the request beside it runs to its end', $bystander, 'BYSTANDER-RAN: 1000');
$t->assertNotContains('and shows no error', $bystander, 'Allowed memory size');

// The next request runs, on this worker.
$t->assertContains('the next request runs', $probe, 'PROBE-RAN');
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('on the worker serving this request', $probe, "worker started: $started");

$t->done();
