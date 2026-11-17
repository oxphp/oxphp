<?php

declare(strict_types=1);

// Shared by the tests that what the engine keeps per thread — the header
// callback of the request running on it, the nesting state of serialize() and
// unserialize() and the comparator of a sort for the fiber running on it — goes
// with a request or a fiber when it is switched out and comes back with it, and
// that the resume puts nothing of a finished request into the resumed one.
//
// Pulled in with require_once for the reason fiber_park_registry.php gives.

/**
 * Sends a request over a fresh connection and hands back the socket unread, so
 * a test can put more than one request on the worker at a time. Reading it is
 * what parks the caller.
 *
 * @return resource
 */
function thread_cell_send(string $path, float $timeout = 8.0)
{
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, $timeout);
    if ($sock === false) {
        throw new \RuntimeException("inner connect failed: $errstr ($errno)");
    }

    stream_set_timeout($sock, (int) ceil($timeout));
    fwrite($sock, "GET $path HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");

    return $sock;
}

/**
 * Reads what thread_cell_send() sent, header block and body apart, since these
 * tests assert on headers.
 *
 * @param resource $sock
 * @return array{headers: string, body: string}
 */
function thread_cell_read($sock, float $timeout = 8.0): array
{
    stream_set_timeout($sock, (int) ceil($timeout));
    $raw = (string) stream_get_contents($sock);
    fclose($sock);

    $split = strpos($raw, "\r\n\r\n");

    return [
        'headers' => $split === false ? $raw : substr($raw, 0, $split),
        'body' => $split === false ? '' : substr($raw, $split + 4),
    ];
}

/**
 * Waits, parked, until $state[$key] is true, and answers whether it got there
 * within $seconds. Each wait is a park of the caller, which is what lets the
 * worker run the request that sets it.
 *
 * @param array<string, mixed> $state
 */
function thread_cell_wait_for(array &$state, string $key, float $seconds = 3.0): bool
{
    $deadline = microtime(true) + $seconds;
    while (($state[$key] ?? false) !== true && microtime(true) < $deadline) {
        oxphp_usleep(10_000);
    }

    return ($state[$key] ?? false) === true;
}

if (!class_exists('ThreadCellFiberHolder', false)) {
    /** Fibers a request leaves suspended for a later request to resume. */
    final class ThreadCellFiberHolder
    {
        public static ?\Fiber $unserialize = null;
        public static ?\Fiber $sort = null;
    }
}

if (!class_exists('ThreadCellUnserializeMarker', false)) {
    /** Filled only by its own __unserialize(), which the engine runs last. */
    final class ThreadCellUnserializeMarker
    {
        /**
         * What the probes unserialized, kept alive. A table a fatal left
         * standing names the values a later call pushed into it, and a probe
         * that resolves a back-reference against that table should read the
         * wrong object, not freed memory.
         *
         * @var list<mixed>
         */
        public static array $kept = [];

        public bool $filled = false;

        /** @return array<string, int> */
        public function __serialize(): array
        {
            return ['v' => 1];
        }

        /** @param array<string, int> $data */
        public function __unserialize(array $data): void
        {
            $this->filled = true;
        }
    }
}

/**
 * Two unserialize() calls whose results say whether they ran on a table of
 * their own: a back-reference must name this call's own object, and the
 * deferred __unserialize() call must run, which the engine does only as the
 * outermost unserialize() on the thread ends.
 *
 * @return array{same: bool, filled: bool}
 */
function thread_cell_unserialize_probe(): array
{
    $o = new \stdClass();
    $o->who = 'probe';
    $pair = unserialize(serialize([$o, $o]));
    $marker = unserialize(serialize(new ThreadCellUnserializeMarker()));
    ThreadCellUnserializeMarker::$kept[] = [$pair, $marker];

    return [
        'same' => is_array($pair) && isset($pair[0], $pair[1]) && $pair[0] === $pair[1] && $pair[1]->who === 'probe',
        'filled' => $marker instanceof ThreadCellUnserializeMarker && $marker->filled,
    ];
}
