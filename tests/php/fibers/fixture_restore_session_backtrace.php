<?php

declare(strict_types=1);

// Inner request for fibers/test_resume_lets_go_of_a_backtrace_its_session_release_leaves.
//
// ?role=parked parks for 800 ms, publishing both edges in $sharedState, then
// says whether a $_SESSION stands where it runs.
//
// ?role=leaver leaves an output buffer that cannot be removed, its handler the
// last thing holding an object whose destructor puts an object of its own in
// $_SESSION. That one's destructor runs when the session state standing on the
// thread is given back, and raises an E_USER_ERROR its own error handler takes
// over, from a frame given a third object as its argument. From PHP 8.5 the
// engine keeps a backtrace of that error, with the arguments of its frames, so
// the third object outlives the call, and when it is released it fills
// $_SESSION again; before 8.5 it goes as the call returns. Every step counts in
// statics the worker keeps across requests.

if (!class_exists('ThreadCellSessionBacktraceLeaver', false)) {
    final class ThreadCellSessionBacktraceLeaver
    {
        public static int $runs = 0;

        public function __destruct()
        {
            ++self::$runs;
            $_SESSION = ['bomb' => new ThreadCellSessionBacktraceBomb()];
        }
    }

    final class ThreadCellSessionBacktraceBomb
    {
        public static int $runs = 0;
        public static bool $handled = false;
        public static ?bool $heldAfterTheCall = null;

        public function __destruct()
        {
            ++self::$runs;
            set_error_handler(static function (int $type): bool {
                if ($type === E_USER_ERROR) {
                    ThreadCellSessionBacktraceBomb::$handled = true;
                }

                return true;
            });
            $released = ThreadCellSessionBacktraceArgument::$released;
            try {
                thread_cell_session_backtrace_raise(new ThreadCellSessionBacktraceArgument());
                self::$heldAfterTheCall = ThreadCellSessionBacktraceArgument::$released === $released;
            } finally {
                restore_error_handler();
            }
        }
    }

    final class ThreadCellSessionBacktraceArgument
    {
        public static int $released = 0;
        public static bool $leftSession = false;

        public function __destruct()
        {
            ++self::$released;
            // Only when what releases it is the backtrace being let go of, not
            // the call returning.
            if (ThreadCellSessionBacktraceBomb::$heldAfterTheCall !== null) {
                self::$leftSession = true;
                $_SESSION = ['left' => 'by-the-argument'];
            }
        }
    }

    function thread_cell_session_backtrace_raise(ThreadCellSessionBacktraceArgument $argument): void
    {
        trigger_error('raised by a session release', E_USER_ERROR);
    }
}

$who = (string) ($_GET['who'] ?? '-');

if (($_GET['role'] ?? '') === 'parked') {
    $sharedState['cell_session_bt_parked'] = true;
    oxphp_usleep(800_000);
    $sharedState['cell_session_bt_parked'] = false;
    echo "BODY:$who;";
    echo isset($_SESSION) ? 'SESSION:stale;' : 'SESSION:none;';

    return;
}

$leaver = new ThreadCellSessionBacktraceLeaver();
ob_start(static function (string $buffer) use ($leaver): string {
    return $buffer;
}, 0, PHP_OUTPUT_HANDLER_STDFLAGS & ~PHP_OUTPUT_HANDLER_REMOVABLE);
unset($leaver);
$sharedState['cell_session_bt_left'] = true;
echo "BODY:$who;";
