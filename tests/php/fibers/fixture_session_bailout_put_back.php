<?php

declare(strict_types=1);

// Puts an object into $_GET whose destructor puts another one back. The first is
// let go of as the request ends, inside its fiber; the second only by the
// worker's own end of the request, after the response has gone out and outside
// any fiber, where no request is left to end. The fatal error its destructor
// ends in leaves the worker's serve loop by a long jump. A fatal in the cycle
// collection the worker runs every hundredth request would not: that one retires
// the worker through the end of the loop, which ends each parked request itself.
//
// The request parks first, so that the worker takes in the request that holds
// the session beside it: once that one has parked with its session open, the
// worker takes in no other until it is closed, and only a request already in
// flight can end the loop while it waits. Before the fatal error the destructor
// notes what that request had noted by then, for the probe to check that the
// session was open when the loop ended.
//
// With ?orphan=<id> the destructor first opens a session of its own under that
// id, as PHP run outside any request can, and leaves it open: the session the
// parked request holds goes back on the thread on top of whatever is standing
// there, so this one has to be given back first or its lock goes the way the
// parked request's would.
//
// The request also arms the code after the oxphp_worker() call in the worker
// entry script, which the long jump skips; the probe says whether it ran.
//
// Driven by tests/scripts/verify_worker_session_closed_after_a_bailout.sh, not
// by a suite. Included inside the worker's handler, where $requestCount is in
// scope.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/after_loop_probe.php';

if (!class_exists('OxphpSessionBailoutAfterTheResponse', false)) {
    final class OxphpSessionBailoutAfterTheResponse
    {
        public function __construct(private string $orphan)
        {
        }

        public function __destruct()
        {
            $status = null;
            if ($this->orphan !== '') {
                session_id($this->orphan);
                session_start();
                $_SESSION['orphan'] = 'written when the orphan was given back';
                $status = session_status();
            }
            $held = is_file('/tmp/oxphp-session-bailout-held')
                ? json_decode((string) file_get_contents('/tmp/oxphp-session-bailout-held'), true)
                : null;
            file_put_contents('/tmp/oxphp-session-bailout-destructor', json_encode([
                'held_id' => $held['id'] ?? null,
                'orphan_status' => $status,
            ]));
            engine_fatal('OxphpSessionBailoutDestructorFatal');
        }
    }

    final class OxphpSessionBailoutPutsBack
    {
        public function __construct(private string $orphan)
        {
        }

        public function __destruct()
        {
            $_GET['put_back'] = new OxphpSessionBailoutAfterTheResponse($this->orphan);
        }
    }
}

OxphpAfterLoop::arm();
$_GET['puts_back'] = new OxphpSessionBailoutPutsBack((string) ($_GET['orphan'] ?? ''));

// Hooked: the request holding the session is taken in while this one waits.
oxphp_sleep(1.5);

// For the orphan the destructor opens: after the response there is no cookie to
// carry its id, and session.* settings refuse to move once headers have gone
// out. They are the thread's, not the request's, so these still stand when the
// destructor runs. Not before the park: the request holding the session reads
// its id from the cookie.
ini_set('session.use_cookies', '0');
ini_set('session.use_strict_mode', '0');

header('Content-Type: application/json');
echo json_encode(['put_back' => true, 'request_count' => $requestCount]);
