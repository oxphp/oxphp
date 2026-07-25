<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_userland_fiber', 'hooks');

// A userland Fiber started inside a worker-mode request fiber runs on a context
// of its own, while OxPHP's "current fiber" pointer still names the outer one.
// Suspending from there would save the continuation into the userland fiber's
// handle and later resume the outer fiber's stale one — silently corrupting the
// control flow of both schedulers. The hook must notice and read natively
// instead.
//
// Without that check this test does not merely fail: the request never produces
// a coherent response, because Fiber::start() returns as though the userland
// fiber had suspended when nothing registered a suspension for it.
$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
$t->assertTrue('probe server listening', $server !== false);
$addr = stream_socket_get_name($server, false);

$client = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
$t->assertTrue('probe client connected', $client !== false);
$peer = stream_socket_accept($server, 3.0);
$t->assertTrue('probe peer accepted', $peer !== false);
stream_set_timeout($client, 3);

// The answer is already on the wire, so the read inside the userland fiber
// returns without waiting — what is under test is which context it returns to.
fwrite($peer, "inside-userland-fiber\n");

$fiber = new Fiber(static function () use ($client): string {
    return (string) fread($client, 64);
});
$fiber->start();

$t->assertTrue('the userland fiber ran to completion', $fiber->isTerminated());
$t->assertContains(
    'the socket read inside the userland fiber returned its data',
    (string) $fiber->getReturn(),
    'inside-userland-fiber'
);

// Stepping aside also means keeping the native timeout contract, which is the
// part of the fallback that has something to get wrong: nothing was waited for,
// so the read must be handed the whole timeout it was given, wait it out, and
// report it exactly as an unhooked read does.
stream_set_timeout($client, 1);
$t0 = microtime(true);
$blocked = new Fiber(static function () use ($client): array {
    $data = fread($client, 64);
    return [$data, stream_get_meta_data($client)['timed_out']];
});
$blocked->start();
$waited = microtime(true) - $t0;
[$data, $timedOut] = $blocked->getReturn();

$t->assertTrue('a read with nothing on the wire yields no data', $data === false || $data === '');
$t->assertTrue('and reports the timeout through stream_get_meta_data()', $timedOut === true);
$t->assertGreaterThan('after waiting its whole timeout', $waited, 0.9);
$t->assertLessThan('and not appreciably longer', $waited, 1.6);

// The outer request fiber must still be intact: a cooperative sleep here goes
// through OxPHP's scheduler, which would be resuming a corrupted context if the
// userland fiber had switched away from underneath it.
$t0 = microtime(true);
oxphp_usleep(50000);
$elapsed = microtime(true) - $t0;
$t->assertGreaterThan('the outer request fiber still suspends and resumes', $elapsed, 0.04);
$t->assertLessThan('and resumes promptly', $elapsed, 1.0);

fclose($client);
fclose($peer);
fclose($server);

$t->done();
