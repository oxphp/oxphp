<?php

declare(strict_types=1);

// Leaves behind a pair of objects that refer to each other and nothing else, so
// that only the cycle collector frees them. A worker runs the collector every
// hundredth request, between requests' slices rather than inside one, so their
// destructor runs where no request is left to end it, and the fatal error it
// ends in ends the worker's serve loop instead. Before the fatal error it notes
// how many requests the worker had taken, for the probe to check that the
// collector ran once the hundredth was in.
//
// With ?orphan=<id> the destructor first opens a session of its own under that
// id, as PHP run outside any request can, and leaves it open: the session the
// parked request holds goes back on the thread on top of whatever is standing
// there, so this one has to be given back first or its lock goes the way the
// parked request's would.
//
// Driven by tests/scripts/verify_worker_session_closed_after_a_bailout.sh, not
// by a suite. Included inside the worker's handler, where $requestCount is in
// scope.

header('Content-Type: application/json');

if (!class_exists('OxphpSessionBailoutCycle', false)) {
    final class OxphpSessionBailoutCycle
    {
        public ?self $other = null;

        public function __construct(private string $orphan)
        {
        }

        public function __destruct()
        {
            $status = null;
            if ($this->orphan !== '') {
                // Between two slices there is no response to carry a cookie,
                // and the id is the probe's to look up.
                ini_set('session.use_cookies', '0');
                ini_set('session.use_strict_mode', '0');
                session_id($this->orphan);
                session_start();
                $_SESSION['orphan'] = 'written when the orphan was given back';
                $status = session_status();
            }
            file_put_contents('/tmp/oxphp-session-bailout-destructor', json_encode([
                'request_count' => \OxPHP\Server\Worker::current()->requestCount(),
                'orphan_status' => $status,
            ]));
            trigger_error('oxphp-session-bailout: a destructor the collector ran ended in a fatal error', E_USER_ERROR);
        }
    }
}

$orphan = (string) ($_GET['orphan'] ?? '');
$first = new OxphpSessionBailoutCycle($orphan);
$second = new OxphpSessionBailoutCycle($orphan);
$first->other = $second;
$second->other = $first;
unset($first, $second);

echo json_encode(['cycle_left' => true, 'request_count' => $requestCount]);
