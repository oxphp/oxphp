<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stream_timeout_worker', 'hooks');

// The timeout contract in worker mode, where the read is parked by the request
// fiber and woken by the HTTP scheduler's own tick and descriptor backoff — a
// different path from the async-pool one that test_stream_timeout_semantics
// covers. The native/hooked pair cannot be compared here because everything in
// this profile runs inside a fiber, so the absolute contract is asserted
// instead.
//
// A listening socket that is never accepted from: the connect completes through
// the kernel backlog, no byte ever arrives, and the read runs into its timeout.
$server = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
$t->assertTrue('probe server listening', $server !== false);
$addr = stream_socket_get_name($server, false);

$client = stream_socket_client("tcp://{$addr}", $errno, $errstr, 3.0);
$t->assertTrue('probe client connected', $client !== false);
stream_set_timeout($client, 1);

$t0 = microtime(true);
$data = fread($client, 16);
$elapsed = microtime(true) - $t0;
$meta = stream_get_meta_data($client);

fclose($client);
fclose($server);

$t->assertSame('the timed-out read reported failure', $data, false);
$t->assertTrue('stream metadata reports timed_out', $meta['timed_out'] === true);
$t->assertGreaterThan('the read waited out its 1s timeout', $elapsed, 0.9);
// Catches the fiber waiting the timeout once and the delegate waiting it again,
// which would land at 2.0s.
$t->assertLessThan('it did not wait its timeout twice', $elapsed, 1.8);

$t->done();
