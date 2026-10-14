<?php

declare(strict_types=1);

use OxPHP\Server\Worker;

// Prepares the next request on this thread for
// test_current_during_request_startup, in the tradautostart profile.
//
// session.auto_start has opened a files session for this request. The script
// puts a WorkerTradWakeProbe into it and closes it, so the session file holds an
// object the next request wakes up while it starts. It then reopens the same
// session through a save handler registered with register_shutdown=false: PHP
// writes that one from the session extension's request shutdown, which runs
// after OxPHP's, and the write asks Worker::current() for an instance there —
// one created after OxPHP's request shutdown, which nothing releases. The write
// records the handle it got into $_GET['out'], so the test can tell it ran.

final class WorkerTradSeedHandler implements SessionHandlerInterface
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
        file_put_contents($this->out, json_encode([
            'worker' => oxphp_worker_id(),
            'late_id' => spl_object_id(Worker::current()),
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
if (!preg_match('#^/tmp/worker-trad-auto-[0-9a-f]+\.json$#', $out)) {
    http_response_code(400);
    exit;
}

$id = session_id();
$_SESSION['probe'] = new WorkerTradWakeProbe();
session_write_close();

session_set_save_handler(new WorkerTradSeedHandler($out), false);
session_id($id);
session_start();
$_SESSION['seeded'] = true;

header('Content-Type: application/json');
echo json_encode([
    'worker' => oxphp_worker_id(),
    'sid' => $id,
]);
