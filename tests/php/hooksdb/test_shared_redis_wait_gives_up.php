<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_redis_wait_gives_up', 'hooksdb');

// What happens when the wait does not succeed — the other half of the guarantee,
// and the one a scenario that always wins the wait never reaches.
//
// The bound is the smaller of max_execution_time and default_socket_timeout, so
// lowering default_socket_timeout to a second makes this fiber give up while the
// holder is still parked. Lowering max_execution_time would do it too, and would
// also arm the request's own deadline, which would end this request rather than
// this call.
//
// For phpredis the answer at that point is an exception rather than the command:
// phpredis does not track connection state, so handing the command to it would put
// it on the wire in the middle of the holder's exchange and the two replies would
// be swapped with nothing raised — the defect itself. PDO and mysqli delegate
// instead, because mysqlnd refuses such a command before sending anything.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_redis_long_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared connection before this one woke',
    isset($sharedState['redis_slow'])
);

// Lowered here on purpose, and the bound must NOT follow it. default_socket_timeout
// is the deadline of a socket operation, and a request narrowing it for an
// fsockopen() of its own is ordinary practice; the wait for a connection is not
// such an operation, and the server reads the value the process started with —
// this image ships two seconds. That distinction is load-bearing in worker mode,
// where a worker serves every request inside one php_request_startup and nothing
// deactivates ini values between them: read as the current value, one library that
// lowered it and did not put it back would shorten this bound for every later
// request on the worker, always towards giving up sooner. So this is both the
// give-up branch and the proof that it is not this call's to shorten — the wait
// below has to come out at the image's two seconds, not at the one asked for here.
//
// Still restored in a finally, because everything else about the value does follow
// the request, and leaving it behind is exactly the defect described above.
$previous = ini_get('default_socket_timeout');
ini_set('default_socket_timeout', '1');

$thrown = '';
$message = '';
$localThrew = '';
$wireThrew = '';
$localWaited = 0.0;
try {
    $started = microtime(true);
    try {
        $sharedState['redis_slow']->get('hooksdb:slow:probe');
    } catch (\Throwable $e) {
        $thrown = get_class($e);
        $message = $e->getMessage();
    }
    $waited = microtime(true) - $started;

    // The list of methods left unguarded is the one place where a wrong entry
    // reopens the defect instead of costing a wait, so both of its sides are
    // checked here against the phpredis actually installed. The byte counters are
    // the client's own bookkeeping and must answer while the connection is held;
    // serverName() reads like their neighbour and does send, so it must not.
    $localStarted = microtime(true);
    try {
        $sharedState['redis_slow']->getTransferredBytes();
    } catch (\Throwable $e) {
        $localThrew = get_class($e);
    }
    $localWaited = microtime(true) - $localStarted;

    try {
        $sharedState['redis_slow']->serverName();
    } catch (\Throwable $e) {
        $wireThrew = get_class($e);
    }
} finally {
    ini_set('default_socket_timeout', (string) $previous);
}

$inner = oxphp_async_await($task);

$t->assertSame('giving up raised RedisException rather than sending the command', $thrown, 'RedisException');
$t->assertContains('the error says the connection was held by another fiber', $message, 'another fiber holds this Redis connection');

// It waited its own bound, and it stopped waiting well before the holder was done:
// one without the other is satisfied by doing nothing at all, or by never giving
// up. The lower bound is above the second this request asked for, so it also fails
// if the wait ever starts following a request-local default_socket_timeout again.
$t->assertGreaterThan('it waited the bound the process started with, not the one this request set', $waited, 1.5);
$t->assertLessThan('it gave up rather than waiting for the holder to finish', $waited, 2.9);

// Both sides of the unguarded-method list, in the same window and on the same
// connection: one answered while the connection was held, the other was stopped.
$t->assertSame('a counter the client keeps for itself answered while the connection was held', $localThrew, '');
$t->assertLessThan('and answered without waiting for the holder', $localWaited, 0.2);
$t->assertSame('a getter that does send was stopped like any other command', $wireThrew, 'RedisException');

// The holder's own exchange survived the refusal untouched — BLPOP on an empty
// list returns an empty result, not someone else's reply.
$t->assertContains('the holding request got the answer to its own command', $inner['body'], 'slow-hold-done:');

$t->done();
