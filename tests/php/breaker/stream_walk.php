<?php

declare(strict_types=1);

// Shared staging for the streams whose client leaves while they are parked, and
// whose held object's destructor then runs as the worker gives back the frames
// the ended request abandoned.
//
// The shape: a request to fixture_stream_walk_dtor.php is sent from here, over a
// socket of this request's own. It makes itself a stream, parks holding an
// object, and is watched until it is parked; then its socket is closed, and the
// server is watched until it has seen the client go. A stream whose client has
// gone is still ended at its next write, so when it resumes into its write it
// is ended there, and its held object is released by the worker's give-back
// rather than by the request's own return. What the destructor does in there is
// the fixture's `dtor` parameter. The tests stage three, one after another.
//
// With `at=session` the fixture holds nothing in its own frame and returns
// after the park without writing; it is its session save handler that holds
// the object and writes, as the worker writes the session after the request.
// That write ends it there, and the object is given back from the save
// handler's frame.
//
// Each step is confirmed before the next one is taken, for the reason
// queued_cancel.php gives: a run that skipped one would pass for the wrong
// reason.
//
// Everything a later request reads is kept in files, not statics: the block
// this serves retires the worker, and the probe after it runs on the
// replacement.
//
// require_once, not require, for the reason breaker_probe.php gives.

const STREAM_WALK_COUNT = 3;

/** Where fixture $id of the $dtor kind records how far it got. */
function stream_walk_stage_file(string $dtor, int $id): string
{
    return "/tmp/oxphp-breaker-walk-$dtor-$id-stage";
}

/** Where its destructor records how far the request had got when it ran. */
function stream_walk_dtor_file(string $dtor, int $id): string
{
    return "/tmp/oxphp-breaker-walk-$dtor-$id-dtor";
}

/** @return string|null the file's contents, or null when it does not exist */
function stream_walk_read(string $file): ?string
{
    return is_file($file) ? (string) file_get_contents($file) : null;
}

/** The server's count of open client connections, or null when /metrics is unreachable. */
function stream_walk_active_connections(): ?int
{
    // With a timeout of its own, as breaker_recycles() has.
    $ctx = stream_context_create(['http' => ['timeout' => 3.0]]);
    $body = @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
    if (!is_string($body) || !preg_match('/^oxphp_active_connections (\d+)$/m', $body, $m)) {
        return null;
    }

    return (int) $m[1];
}

/**
 * Polls until $until returns true, for at most $seconds.
 *
 * oxphp_usleep, not usleep: this profile runs no hooks, so a native sleep would
 * block the worker, and the fixtures this waits for could never run.
 */
function stream_walk_wait(callable $until, float $seconds): bool
{
    $deadline = microtime(true) + $seconds;
    do {
        if ($until()) {
            return true;
        }
        oxphp_usleep(20_000);
    } while (microtime(true) < $deadline);

    return false;
}

/**
 * Removes what an earlier run left for the fixtures $ids of the $dtor kind —
 * 1 to STREAM_WALK_COUNT unless given. Done up front for all of them, not
 * stream by stream: a block that stops early leaves the later streams
 * unstaged, and a marker an earlier run left for one of those would answer its
 * assertion instead.
 *
 * @param list<int>|null $ids
 */
function stream_walk_clear(string $dtor, ?array $ids = null): void
{
    foreach ($ids ?? range(1, STREAM_WALK_COUNT) as $id) {
        foreach ([stream_walk_stage_file($dtor, $id), stream_walk_dtor_file($dtor, $id)] as $file) {
            if (is_file($file)) {
                unlink($file);
            }
        }
    }
}

/**
 * Stages fixture $id of the $dtor kind and waits for its destructor. Returns a
 * failure description for the step that did not happen, or null when every step
 * did.
 *
 * One stream at a time, never several side by side. A fatal flags every object
 * alive on the worker as already destructed, the objects other requests on it
 * are holding included, so a stream staged beside one whose destructor fatals
 * would have its own destructor skipped, and would never get to fatal at all.
 *
 * Expects stream_walk_clear() to have run for the block. A step that fails
 * leaves the rest undone: this is also called by a request
 * that cannot report, and what it returns is all its caller has.
 *
 * With $parkThrough the destructor is not polled for: this request parks once,
 * past the fixture's own park, and only then looks. For the stream whose fatal
 * retires the worker. Polled, this request's short sleep can run out at the
 * same tick as the fixture's, and resumed right behind it in that tick it
 * would find the destructor's file, return, and finish its request cleanly —
 * which resets the count of consecutive failures before the worker checks it.
 */
function stream_walk_stage(string $dtor, int $id, bool $parkThrough = false, string $at = 'handler'): ?string
{
    $stageFile = stream_walk_stage_file($dtor, $id);
    $dtorFile = stream_walk_dtor_file($dtor, $id);

    $before = stream_walk_active_connections();
    if ($before === null) {
        return '/metrics does not report the open client connections';
    }

    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "stream $id: socket did not connect: $errstr";
    }
    // No body, so the server keeps reading the connection while the request runs
    // and sees the close as soon as it happens.
    fwrite($sock, "GET /tests/breaker/fixture_stream_walk_dtor.php?id=$id&dtor=$dtor&at=$at HTTP/1.1\r\n"
        . "Host: 127.0.0.1\r\n\r\n");

    // Closed only once it is running: a request whose client leaves while it is
    // still queued is dropped before it starts.
    $parked = static fn (): bool => stream_walk_read($stageFile) === 'parked';
    if (!stream_walk_wait($parked, 3.0)) {
        return "stream $id: did not park";
    }
    fclose($sock);

    // The count drops once the connection's request has been marked cancelled
    // with it. Still parked at that reading means the cancellation reached it
    // while it was parked, not after it had moved on.
    $gone = stream_walk_wait(
        static fn (): bool => stream_walk_active_connections() === $before && $parked(),
        3.0
    );
    if (!$gone) {
        return "stream $id: the server did not see its client leave while it was parked";
    }

    // Marking a request cancelled also raises the worker's interrupt flag, which
    // is acted on by whichever request runs PHP next. The reading above already
    // ran calls in this request after it was raised; this is one more, so the
    // flag is spent here rather than in the fixture as it resumes — there it
    // would end the request at the interrupt, before it reached its write, and
    // the destructor would never run in the give-back this is about.
    microtime(true);

    if ($parkThrough) {
        // Twice the fixture's whole park, which is already under way.
        oxphp_sleep(4.0);
        return is_file($dtorFile) ? null : "stream $id: its destructor did not run";
    }
    if (!stream_walk_wait(static fn (): bool => is_file($dtorFile), 6.0)) {
        return "stream $id: its destructor did not run";
    }

    return null;
}
