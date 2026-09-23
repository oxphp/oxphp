<?php

declare(strict_types=1);

// A request whose session save handler fatals as the session is written.
//
// The worker writes a request's session after the request has returned, once
// its shutdown functions have run, the way PHP's own request shutdown does. A
// fatal there is the request coming apart like any other — the engine state it
// leaves is the same as a fatal in the handler's — so it counts toward the
// breaker. Three in a row retire the worker; the probe after them reads that.
//
// The fatal is a redeclaration reached through require — see
// breaker_redeclare.php for why a method cannot contain a `class` statement.

// Declared once per worker: a class outlives the request that declared it.
if (!class_exists('BreakerSessionWriteFatal', false)) {
final class BreakerSessionWriteFatal implements SessionHandlerInterface
{
    public function open(string $path, string $name): bool
    {
        return true;
    }

    public function close(): bool
    {
        return true;
    }

    public function read(string $id): string
    {
        return '';
    }

    public function write(string $id, string $data): bool
    {
        require __DIR__ . '/breaker_redeclare.php';
        require __DIR__ . '/breaker_redeclare.php';

        return true;
    }

    public function destroy(string $id): bool
    {
        return true;
    }

    public function gc(int $max_lifetime): int|false
    {
        return 0;
    }
}
}

session_set_save_handler(new BreakerSessionWriteFatal(), false);
session_id('oxphpbreakersessionwritefatal');
session_start();
$_SESSION['n'] = 1;

echo "the handler itself ends without incident\n";
