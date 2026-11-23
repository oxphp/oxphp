<?php

declare(strict_types=1);

// Shared by the net and curl tests. Not a test itself, so no suite lists it. Only
// the request side uses it: an async task runs in an environment of its own that
// has none of these functions, so what a task needs it gets as arguments.

/** The address of the TLS peer, resolved once by the request so that the tasks
 *  spend their time on the waits under test and not on a name lookup. */
function net_peer_ip(): string
{
    $host = getenv('NET_PEER') ?: 'hooks-netpeer';
    $ip = gethostbyname($host);
    if ($ip === $host) {
        throw new RuntimeException("cannot resolve the TLS peer '{$host}'");
    }
    return $ip;
}

/** An address on the peer's own subnet that no container holds. A connect to it
 *  gets no answer and no refusal — the kernel spends its time asking who has the
 *  address — so the connect lasts exactly as long as the timeout it is given.
 *  Unlike a routed address that nobody answers, it behaves the same on every
 *  Docker host.
 *
 *  That holds only while the kernel is still asking: about three seconds after
 *  the first probe it gives up on the address, and a connect that is running at
 *  that moment fails early with EHOSTUNREACH instead of timing out. Whoever
 *  connected there a moment ago has started that clock, so a test that checks how
 *  long a connect lasted takes an address of its own with $slot, and keeps the
 *  connects it makes there within a couple of seconds. */
function net_silent_address(int $slot = 0): string
{
    $parts = explode('.', net_peer_ip());
    $parts[3] = (string) (250 - $slot);
    return implode('.', $parts);
}

/** Runs fixture_curl_hold.php, or another fixture of this directory, as a second
 *  request on this worker — through a task, because this request has to stay
 *  free to act on the handle the holder parks on — and returns the task. Its
 *  result is the fixture's JSON, decoded, or `['transport' => …]` when no answer
 *  came back. */
function curl_hold_start(string $query, string $fixture = 'fixture_curl_hold.php')
{
    return oxphp_async(function (string $query, string $fixture): array {
        $sock = @stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock === false) {
            return ['transport' => "connect failed: {$errstr} ({$errno})"];
        }
        stream_set_timeout($sock, 12);
        fwrite($sock, "GET /tests/hooks/{$fixture}?{$query} HTTP/1.0\r\n"
            . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
        $response = (string) stream_get_contents($sock);
        fclose($sock);

        $split = strpos($response, "\r\n\r\n");
        $decoded = $split === false ? null : json_decode(substr($response, $split + 4), true);
        return is_array($decoded) ? $decoded : ['transport' => $response];
    }, $query, $fixture);
}

/** Waits up to three seconds for the holder to reach its transfer, then a little
 *  longer so that it is parked inside it rather than about to be. */
function curl_hold_parked(array &$sharedState): bool
{
    $deadline = microtime(true) + 3.0;
    while (!($sharedState['curl_parked'] ?? false) && microtime(true) < $deadline) {
        oxphp_usleep(20_000);
    }
    if (!($sharedState['curl_parked'] ?? false)) {
        return false;
    }
    oxphp_usleep(200_000);
    return (bool) ($sharedState['curl_parked'] ?? false);
}
