<?php

/**
 * Shared setup for the revolt profile.
 *
 * Pulled in with require_once, so in worker mode the statics below outlive a
 * single request and are shared by every request the worker serves — which is
 * exactly the vantage point these tests need: two multiplexed requests must be
 * able to look at one FiberLocal instance and one EventLoop driver and see
 * their own state, not each other's.
 */

declare(strict_types=1);

require_once '/var/www/html/revolt/autoload.php';

use Revolt\EventLoop;
use Revolt\EventLoop\FiberLocal;

/**
 * One FiberLocal instance for the whole worker. Its per-context storage is what
 * is under test, not the instance itself, so sharing the instance is the point.
 */
function revolt_shared_local(): FiberLocal
{
    static $local = null;

    return $local ??= new FiberLocal(static fn (): string => 'unset');
}

/**
 * What one request can observe about the loop state it was handed.
 *
 * The suspension is pinned for the worker's lifetime before its id is read.
 * Object ids are reused after collection, and the driver only keeps weak
 * references, so without pinning the inner request's suspension could be freed
 * when that request ends and its id handed to an unrelated object the outer
 * request allocates — two ids would then match for the wrong reason. Pinned,
 * both objects are alive at the moment the ids are compared, so equal ids mean
 * one object.
 *
 * @return array{suspension_id: int, local_seen: string, is_userland_fiber: bool}
 */
function revolt_probe(): array
{
    static $pinned = [];

    $suspension = EventLoop::getSuspension();
    $pinned[] = $suspension;

    return [
        'suspension_id' => spl_object_id($suspension),
        'local_seen' => (string) revolt_shared_local()->get(),
        'is_userland_fiber' => \Fiber::getCurrent() !== null,
    ];
}

/**
 * Issues a request to $path over a fresh connection and returns its body.
 *
 * With PHP_WORKERS=1 the response can only be produced by the worker thread
 * running this call, so a body coming back proves the two requests were
 * multiplexed on one thread. The read is what parks this fiber (RUNTIME_HOOKS
 * covers a blocking read on a tcp:// stream), which is what gives the worker
 * the opportunity to pick the inner request up.
 */
function revolt_inner_request(string $path, float $timeout = 5.0): string
{
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, $timeout);
    if ($sock === false) {
        throw new \RuntimeException("inner connect failed: $errstr ($errno)");
    }

    stream_set_timeout($sock, (int) ceil($timeout));
    fwrite($sock, "GET $path HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");

    $raw = (string) stream_get_contents($sock);
    fclose($sock);

    $split = strpos($raw, "\r\n\r\n");

    return $split === false ? $raw : substr($raw, $split + 4);
}
