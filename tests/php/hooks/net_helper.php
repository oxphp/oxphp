<?php

declare(strict_types=1);

// Shared by the net tests. Not a test itself, so no suite lists it. Only the
// request side uses it: an async task runs in an environment of its own that has
// none of these functions, so what a task needs it gets as arguments.

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
