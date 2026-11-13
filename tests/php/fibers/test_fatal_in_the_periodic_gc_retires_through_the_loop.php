<?php

declare(strict_types=1);

require_once __DIR__ . '/after_loop_probe.php';

// A destructor run by the cycle collection a worker makes every hundredth
// request ends in a fatal error. The collection runs between requests, with
// nothing current to fail, so the worker retires — and leaves its loop the way
// any retire does: the requests still parked on it are each answered, and the
// entry script runs on past the oxphp_worker() call.
//
// This request is the one parked: it leaves the cycle, then sends the inner
// requests that bring the worker's count to the next hundred, and waits on
// them. Its answer is the suite line's expected status. What the code after
// the loop found is read by fibers/..._probe, on the next line.

$base = sys_get_temp_dir() . '/oxphp-periodic-gc-fatal';
if (is_file("$base.ran")) {
    unlink("$base.ran");
}
OxphpAfterLoop::arm();

if (!class_exists('OxphpPeriodicGcFatalCycle', false)) {
    final class OxphpPeriodicGcFatalCycle
    {
        public ?self $self = null;

        public function __construct(private string $marker)
        {
        }

        public function __destruct()
        {
            file_put_contents($this->marker, 'ran');
            // A compile error, which no error handler sees.
            eval('abstract class OxphpPeriodicGcFatal { abstract public function f() {} }');
        }
    }
}

$cycle = new OxphpPeriodicGcFatalCycle("$base.ran");
$cycle->self = $cycle;
unset($cycle);

// The collection runs once the count is a multiple of the interval; the count
// includes this request.
$count = OxPHP\Server\Worker::current()->requestCount();
$padding = (100 - $count % 100) % 100;
for ($i = 0; $i < $padding; $i++) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        echo "FAIL: inner request $i did not connect: $errstr\n";
        return;
    }
    fwrite($sock, "GET /?action=is_worker HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    // Hooked: parks this request so the worker serves the inner one.
    stream_get_contents($sock);
    fclose($sock);
}

// With no padding, this request is the hundredth itself: the collection runs on
// the loop's first pass after it parks here.
usleep(500_000);

echo "FAIL: still running after the collection at request " . ($count + $padding) . "\n";
