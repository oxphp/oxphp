<?php

declare(strict_types=1);

// Inner self-request for fibers/test_channel_wait_in_a_destructor_between_requests_throws.
//
// Dispatches a task that has captured, by reference, an object whose destructor
// receives from an empty channel without a timeout, and ends while the task
// still runs. A running task is cancelled as the request ends and its closure
// released once it has stopped, between two slices of whatever the worker serves
// next: on the worker's own frame, where no request runs and where one parked on
// the worker cannot run until the wait ends.
//
// With ?catch=1 the destructor writes what recv() did to the marker; without, it
// writes only that it started, and lets an exception go.

$run = preg_replace('/[^0-9a-f]/', '', (string) ($_GET['run'] ?? ''));
$base = sys_get_temp_dir() . '/oxphp-channel-left-' . $run;
$catch = ($_GET['catch'] ?? '') === '1';

$box = null;
oxphp_async(static function () use (&$box, $base): int {
    file_put_contents("$base.task", 'running');
    usleep(300_000);

    return 1;
});
for ($i = 0; $i < 100 && !is_file("$base.task"); $i++) {
    usleep(20_000);
}
if (!is_file("$base.task")) {
    echo "TASK-NEVER-RAN\n";
    return;
}

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file.
$box = new class (new OxPHP\Shared\Channel(1), "$base.ran", $catch) {
    public function __construct(
        private OxPHP\Shared\Channel $channel,
        private string $marker,
        private bool $catch,
    ) {
    }

    public function __destruct()
    {
        file_put_contents($this->marker, 'entered');
        if (!$this->catch) {
            $this->channel->recv();

            return;
        }
        try {
            $this->channel->recv();
            $outcome = 'received';
        } catch (\Throwable $e) {
            $outcome = get_class($e) . ': ' . $e->getMessage();
        }
        file_put_contents($this->marker, $outcome);
    }
};
// The closure is now the only holder.
unset($box);
echo "DISPATCHED\n";
