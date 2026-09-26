<?php

declare(strict_types=1);

// Holds a shared connection mid-exchange for test_shared_conn_outside_fiber_refused,
// in one of three shapes chosen by ?mode=:
//
//   redis      — through phpredis, parked in its reply;
//   redis-idle — through phpredis, holding the client-level claim but parked on a
//                sleep, not on the connection;
//   raw        — through a hand-written RESP exchange on a plain tcp:// stream,
//                which has no client level at all.
//
// The two that are mid-exchange block on an empty list, so Redis holds the reply back and the request stays
// parked in its read for the whole window. Each has its own connection under its
// own key, so the two shapes cannot answer for each other.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
$mode = $_GET['mode'] ?? '';

try {
    if ($mode === 'redis') {
        $redis = new Redis();
        // Read timeout above the BLPOP timeout, or the socket gives up first.
        $redis->connect($host, 6379, 3.0, null, 0, 15.0);
        $sharedState['outside_fiber_redis'] = $redis;

        // Printed verbatim: phpredis turns a reply meant for someone else into a
        // falsy result, so accepting anything falsy would accept the defect.
        $popped = $redis->blPop(['hooksdb:outside:empty'], 5);
        $sharedState['outside_fiber_redis_done'] = true;
        echo 'redis-hold-done:' . var_export($popped, true);
    } elseif ($mode === 'redis-idle') {
        // Holds the client-level claim without being in an exchange: one command,
        // then a wait on something else. The claim lasts to the end of this request,
        // but nothing is on the wire, so a command from elsewhere is safe here.
        $redis = new Redis();
        $redis->connect($host, 6379, 3.0, null, 0, 15.0);
        $sharedState['outside_fiber_redis_idle'] = $redis;
        $redis->get('hooksdb:outside:probe');

        oxphp_usleep(5_000_000);
        $sharedState['outside_fiber_redis_idle_done'] = true;
        echo 'redis-idle-done';
    } elseif ($mode === 'raw') {
        $sock = stream_socket_client("tcp://{$host}:6379", $errno, $errstr, 3.0);
        if ($sock === false) {
            echo "raw-hold-failed:connect {$errstr} ({$errno})";
            return;
        }
        stream_set_timeout($sock, 15);
        $sharedState['outside_fiber_raw'] = $sock;

        $cmd = '';
        foreach (['BLPOP', 'hooksdb:outside:raw', '5'] as $arg) {
            $cmd .= '$' . strlen($arg) . "\r\n{$arg}\r\n";
        }
        fwrite($sock, "*3\r\n{$cmd}");
        // Everything that arrives in the first read after the pop times out. A nil
        // array is `*-1\r\n`; anything past it is a reply to someone else's command.
        $reply = (string) fread($sock, 4096);
        $sharedState['outside_fiber_raw_done'] = true;
        echo 'raw-hold-done:' . json_encode($reply);
    } else {
        echo 'hold-failed:unknown mode';
    }
} catch (\Throwable $e) {
    echo 'hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
