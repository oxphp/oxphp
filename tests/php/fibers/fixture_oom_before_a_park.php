<?php

declare(strict_types=1);

// Inner self-request for fibers/test_oom_before_a_park_stays_with_its_request.
//
// ?step=oom&park=... runs past memory_limit and then waits about three seconds,
// in the way `park` names:
//   sleep — a hooked sleep()
//   read  — a read on a socket pair nothing is written to
//   await — oxphp_async_await() on a task that sleeps
//   any   — oxphp_async_await_any() on a task that sleeps
// From PHP 8.4.27 / 8.5.12 the engine lets the allocation through and reports
// the error later, at the next point that checks for it — among them the return
// from an internal function and any jump the script takes, past an `if` it does
// not enter included. So the allocation is the last thing before the wait, in
// the branch that makes it, and everything the wait needs is set up ahead of
// it. The error is displayed, so the response shows which request it was
// reported in, and WOKE says the wait ran to its end.
//
// ?step=bystander waits a second first — so that, whichever of the two this
// worker started first, it is parked while the other one runs out — then says
// it ran.
//
// ?step=probe is the request after both: it says it ran, and on which worker.

$step = $_GET['step'] ?? 'oom';

if ($step === 'bystander') {
    sleep(1);
    echo "BYSTANDER-RAN: 1000\n";
    return;
}

if ($step === 'probe') {
    echo "PROBE-RAN\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    return;
}

$park = $_GET['park'] ?? 'sleep';

ini_set('display_errors', '1');
ini_set('log_errors', '0');
ini_set('memory_limit', '64M');

$pair = null;
$promise = null;
if ($park === 'read') {
    $pair = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    stream_set_timeout($pair[0], 3);
} elseif ($park === 'await' || $park === 'any') {
    $promise = oxphp_async(static function (): void {
        sleep(3);
    });
}

// Doubled, about 72 MiB in all: past the 64 MiB limit, and well short of the
// hard limit at twice that, which would end the server rather than the request.
$s = str_repeat('x', 24 << 20);

if ($park === 'sleep') {
    $doubled = $s . $s;
    sleep(3);
} elseif ($park === 'read') {
    $doubled = $s . $s;
    fread($pair[0], 1);
} elseif ($park === 'await') {
    $doubled = $s . $s;
    oxphp_async_await($promise, 10.0);
} else {
    $doubled = $s . $s;
    oxphp_async_await_any([$promise], 10.0);
}

echo "WOKE\n";
