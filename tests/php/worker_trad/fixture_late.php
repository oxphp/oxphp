<?php

declare(strict_types=1);

use OxPHP\Server\Worker;

// Calls Worker::current() once more after OxPHP has finished with the request.
//
// A session save handler registered with register_shutdown=false is written by
// the session extension's own request shutdown. PHP shuts extensions down in
// the reverse of the order it started them, and the session extension is built
// into PHP while OxPHP's is loaded from an ini file, so that write runs after
// OxPHP's request shutdown. From there the handler records three things into
// $_GET['out'] for the test to read once the thread has served its next request:
//
// - whether the instance this script got had already been released by then. The
//   script keeps no reference of its own — only a WeakMap entry, which goes away
//   when the object is freed — so the instance is freed exactly when OxPHP lets
//   go of it;
// - the handle of the instance Worker::current() returned to a destructor that
//   ran while OxPHP was releasing the script's instance. Freeing an object frees
//   the values a WeakMap holds under it, and a LateDtorProbe is created by the
//   output handler — after PHP has run the request's destructors, so its own
//   still runs when it is freed. It must be a new object: once PHP has run a
//   request's destructors it stops reusing handles, so a new object never gets
//   the handle of one released before it, and getting the script's handle back
//   means the call read the instance being released;
// - the handle of the instance Worker::current() returns from inside the write,
//   which must not be the script's either. It is the one the destructor got:
//   OxPHP empties its slot before the release, and the first call after that
//   creates the instance every later call gets back. Nothing releases that
//   instance before PHP tears the request down, and the next request on the
//   thread must not be handed it.

final class LateProbe
{
    public static ?WeakMap $seen = null;
    public static int $scriptId = 0;
    public static ?int $dtorId = null;
}

final class LateDtorProbe
{
    public function __destruct()
    {
        LateProbe::$dtorId = spl_object_id(Worker::current());
    }
}

final class LateHandler implements SessionHandlerInterface
{
    public function __construct(private readonly string $out)
    {
    }

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
        $released = count(LateProbe::$seen) === 0;
        $late = Worker::current();
        file_put_contents($this->out, json_encode([
            'worker' => oxphp_worker_id(),
            'released_before' => $released,
            'script_id' => LateProbe::$scriptId,
            'dtor_id' => LateProbe::$dtorId,
            'late_id' => spl_object_id($late),
        ]));
        return true;
    }

    public function destroy(string $id): bool
    {
        return true;
    }

    public function gc(int $max_lifetime): int
    {
        return 0;
    }
}

$out = (string) ($_GET['out'] ?? '');
if (!preg_match('#^/tmp/worker-trad-[0-9a-f]+\.json$#', $out)) {
    http_response_code(400);
    exit;
}

LateProbe::$seen = new WeakMap();
LateProbe::$seen[Worker::current()] = true;
LateProbe::$scriptId = spl_object_id(Worker::current());

// PHP calls this when it flushes the output buffers in its request shutdown.
ob_start(static function (string $buffer): string {
    LateProbe::$seen[Worker::current()] = new LateDtorProbe();
    return $buffer;
});

session_set_save_handler(new LateHandler($out), false);
session_start();
$_SESSION['touched'] = true;

header('Content-Type: application/json');
echo json_encode([
    'worker' => oxphp_worker_id(),
    'tracked' => count(LateProbe::$seen),
]);
