<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_generic_request_multiplex', 'hooks');

// A read on a local socket — one end of stream_socket_pair(), the client end of a
// unix:// connection, then a bound udp:// and a bound udg:// socket — that parks
// this request fiber. PHP_WORKERS=1 makes it
// decisive: the answer can only be written by a request this same worker thread
// has to serve, so a read that parks lets that request run and a read that holds
// the thread waits out its own timeout with nobody left to write.
//
// These sockets are not tcp:// streams, and each of those kinds carries an ops table
// of its own that the streams hook does not reach; what they wait in is the
// engine's generic socket read, which is the net category's to park.
function net_ask_inner(string $which): array
{
    $http = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($http === false) {
        return [false, 0.0, "inner connect failed: {$errstr}"];
    }
    fwrite($http, "GET /tests/hooks/fixture_inner_net_write.php?which={$which} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    return [$http, microtime(true), ''];
}

// ---- stream_socket_pair() ----
$pair = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
$t->assertTrue('the socket pair was created', is_array($pair));
$sharedState['net_pair_writer'] = $pair[1];
stream_set_timeout($pair[0], 2);

[$http, $t0, $err] = net_ask_inner('pair');
$t->assertTrue('the inner request was sent', $http !== false);
$data = (string) fread($pair[0], 16);
$elapsed = microtime(true) - $t0;
$t->assertSame('the pair read got the answer the inner request wrote', $data, 'net-done');
$t->assertGreaterThan('and it really waited for it', $elapsed, 0.9);
$t->assertLessThan('without waiting out its own two-second timeout', $elapsed, 1.8);
$inner = (string) stream_get_contents($http);
$t->assertContains('the inner request finished as usual', $inner, 'inner-done');
fclose($http);
unset($sharedState['net_pair_writer']);
fclose($pair[0]);
fclose($pair[1]);

// ---- unix:// ----
$path = sys_get_temp_dir() . '/oxphp_net_' . getmypid() . '_' . uniqid('', true) . '.sock';
$server = stream_socket_server("unix://{$path}", $errno, $errstr);
$t->assertTrue('the unix listener was created', $server !== false);
$client = stream_socket_client("unix://{$path}", $errno, $errstr, 3.0);
$t->assertTrue('the unix client connected', $client !== false);
$peer = stream_socket_accept($server, 3.0);
$t->assertTrue('the unix peer was accepted', $peer !== false);
$sharedState['net_unix_writer'] = $peer;
stream_set_timeout($client, 2);

[$http, $t0, $err] = net_ask_inner('unix');
$t->assertTrue('the second inner request was sent', $http !== false);
$data = (string) fread($client, 16);
$elapsed = microtime(true) - $t0;
$t->assertSame('the unix read got the answer the inner request wrote', $data, 'net-done');
$t->assertGreaterThan('and it really waited for it', $elapsed, 0.9);
$t->assertLessThan('without waiting out its own two-second timeout', $elapsed, 1.8);
$inner = (string) stream_get_contents($http);
$t->assertContains('the second inner request finished as usual', $inner, 'inner-done');
fclose($http);
unset($sharedState['net_unix_writer']);
fclose($client);
fclose($peer);
fclose($server);
@unlink($path);

// ---- udp:// ---- a bound datagram socket read, the answer sent from a client
// stream the request hands to the inner one
$udp = stream_socket_server('udp://127.0.0.1:0', $errno, $errstr, STREAM_SERVER_BIND);
$t->assertTrue('the udp socket was bound', $udp !== false);
$udpName = (string) stream_socket_get_name($udp, false);
$udpPort = (int) substr($udpName, (int) strrpos($udpName, ':') + 1);
$udpClient = stream_socket_client("udp://127.0.0.1:{$udpPort}", $errno, $errstr, 3.0);
$t->assertTrue('the udp client was created', $udpClient !== false);
$sharedState['net_udp_writer'] = $udpClient;
stream_set_timeout($udp, 2);

[$http, $t0, $err] = net_ask_inner('udp');
$t->assertTrue('the third inner request was sent', $http !== false);
$data = (string) fread($udp, 16);
$elapsed = microtime(true) - $t0;
$t->assertSame('the udp read got the answer the inner request wrote', $data, 'net-done');
$t->assertGreaterThan('and it really waited for it', $elapsed, 0.9);
$t->assertLessThan('without waiting out its own two-second timeout', $elapsed, 1.8);
$inner = (string) stream_get_contents($http);
$t->assertContains('the third inner request finished as usual', $inner, 'inner-done');
fclose($http);
unset($sharedState['net_udp_writer']);
fclose($udpClient);
fclose($udp);

// ---- udg:// ---- the same over a unix datagram socket
$dgPath = sys_get_temp_dir() . '/oxphp_net_dg_' . getmypid() . '_' . uniqid('', true) . '.sock';
$udg = stream_socket_server("udg://{$dgPath}", $errno, $errstr, STREAM_SERVER_BIND);
$t->assertTrue('the udg socket was bound', $udg !== false);
$udgClient = stream_socket_client("udg://{$dgPath}", $errno, $errstr, 3.0);
$t->assertTrue('the udg client was created', $udgClient !== false);
$sharedState['net_udg_writer'] = $udgClient;
stream_set_timeout($udg, 2);

[$http, $t0, $err] = net_ask_inner('udg');
$t->assertTrue('the fourth inner request was sent', $http !== false);
$data = (string) fread($udg, 16);
$elapsed = microtime(true) - $t0;
$t->assertSame('the udg read got the answer the inner request wrote', $data, 'net-done');
$t->assertGreaterThan('and it really waited for it', $elapsed, 0.9);
$t->assertLessThan('without waiting out its own two-second timeout', $elapsed, 1.8);
$inner = (string) stream_get_contents($http);
$t->assertContains('the fourth inner request finished as usual', $inner, 'inner-done');
fclose($http);
unset($sharedState['net_udg_writer']);
fclose($udgClient);
fclose($udg);
@unlink($dgPath);

$t->done();
