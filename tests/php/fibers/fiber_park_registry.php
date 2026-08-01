<?php

/**
 * Shared setup for the foreign-resume test.
 *
 * Pulled in with require_once, so in worker mode the state below outlives a
 * single request and is shared by every request the worker serves — which is the
 * vantage point this test needs: one request has to hand its own \Fiber to
 * another request running on the same worker thread, the way a library that
 * stores \Fiber::getCurrent() in a registry would.
 */

declare(strict_types=1);

/**
 * The one place the outer request leaves its own fiber for the inner one.
 */
final class ParkedRequestFiber
{
    private static ?\Fiber $fiber = null;

    public static function set(\Fiber $fiber): void
    {
        self::$fiber = $fiber;
    }

    public static function get(): ?\Fiber
    {
        return self::$fiber;
    }
}

/**
 * Runs $fn and reports what came back out of it, as "Class: message" or the
 * literal 'no-throw'. Both are strings so the result survives the JSON hop
 * between the two requests.
 */
function fiber_attempt(callable $fn): string
{
    try {
        $fn();

        return 'no-throw';
    } catch (\Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
}

/**
 * Issues a request to $path over a fresh connection and returns its body.
 *
 * With PHP_WORKERS=1 the response can only be produced by the worker thread
 * running this call, so a body coming back proves the two requests were
 * multiplexed on one thread. The read is what parks this fiber (RUNTIME_HOOKS
 * covers a blocking read on a tcp:// stream), which both gives the worker the
 * opportunity to pick the inner request up and leaves this one parked while it
 * runs.
 */
function fiber_inner_request(string $path, float $timeout = 5.0): string
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
