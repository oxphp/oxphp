<?php

declare(strict_types=1);

// Fixture for the bound-`$this` orphan test. A non-static closure defined inside
// a method captures `$this`, so the dispatched task runs bound to an object. The
// request returns without awaiting it, so the end of the request is what cancels
// the task.
//
// The task's `$this` must be an object the task alone owns — not one that lives
// in this request's memory and is torn down while the task is still running.
// The task deliberately outlives the request: it catches the request-end
// cancellation, then for a few seconds keeps checking that `$this` is still an
// `OxPHP\Async\BorrowedProxy` with its class intact, over a window that spans
// past the point the request's heap is gone. It records:
//   `<marker>.started`   as the task starts, from inside the `try` that catches
//                        the cancellation,
//   `<marker>.caught_at` when the cancellation is delivered,
//   `<marker>.result`    "ok", or the first anomaly it saw in `$this`,
//   `<marker>.ended_at`  when it stops.
//
// The fixture also captures any error raised while dispatching into
// `<marker>.dispatch_err`, so the test can assert that dispatching a bound
// closure raises no deprecation (the proxy gets no dynamic `promiseId`
// property).
$id = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['id'] ?? ''));
$base = sys_get_temp_dir() . "/oxphp_thisbound_{$id}";

final class OrphanDispatcher
{
    public function dispatch(string $base): void
    {
        $task = function () use ($base): void {
            $start = microtime(true);
            try {
                // Written inside the `try`: the request returns, and its end
                // cancels the task, as soon as this file exists — the file is
                // there before the call even returns — and the cancellation
                // lands on the next opcode boundary. Written before the `try`,
                // that boundary can be this call's return, outside the `catch`.
                file_put_contents("$base.started", '1');
                while (microtime(true) - $start < 20.0) {
                    // CPU-bound; only the request-end interrupt breaks this.
                }
            } catch (\Throwable) {
                file_put_contents("$base.caught_at", (string) microtime(true));
                // Keep reading `$this` for a window that reaches past the drain
                // budget — i.e. past the point the dispatching request's heap is
                // gone. A `$this` the pool owns stays its own class the whole
                // time; a `$this` that pointed into the request's freed memory
                // need not. `get_class()` invokes no magic method, so it reads
                // the object directly.
                $verdict = 'ok';
                try {
                    $deadline = microtime(true) + 7.0;
                    while (microtime(true) < $deadline) {
                        $cls = get_class($this);
                        if ($cls !== 'OxPHP\\Async\\BorrowedProxy') {
                            $verdict = "class=$cls";
                            break;
                        }
                        usleep(200000);
                    }
                } catch (\Throwable $e) {
                    $verdict = 'threw ' . get_class($e) . ': ' . $e->getMessage();
                }
                file_put_contents("$base.result", $verdict);
                file_put_contents("$base.ended_at", (string) microtime(true));
                return;
            }
            file_put_contents("$base.result", 'not cancelled');
            file_put_contents("$base.ended_at", (string) microtime(true));
        };

        $errors = [];
        set_error_handler(static function (int $no, string $str) use (&$errors): bool {
            $errors[] = $str;
            return true;
        });
        oxphp_async($task);
        restore_error_handler();
        file_put_contents("$base.dispatch_err", implode("\n", $errors));

        // Return only once the task is actually running, so the request end
        // cancels a live task rather than one still queued.
        $deadline = microtime(true) + 2.0;
        while (!file_exists("$base.started") && microtime(true) < $deadline) {
            usleep(5000);
            clearstatcache();
        }
    }
}

(new OrphanDispatcher())->dispatch($base);
echo 'orphan-dispatched';
