<?php

declare(strict_types=1);

// Inner request for fibers/test_restore_runs_no_destructor_into_the_resumed_request.
//
// ?role=parked parks for 800 ms, publishing both edges in $sharedState, then
// sets a header, which it can only do while its headers have not gone out.
//
// ?role=leaver leaves an output buffer that cannot be removed, so the end of
// the request cannot take it off. The buffer's handler is the last thing
// holding an object whose destructor writes, names the request whose $_GET it
// reads and registers a shutdown function holding an object of its own, and
// counts its runs, and that object's release, in statics the worker keeps
// across requests. The object goes when the buffer is freed, by
// whatever frees what this request left standing.

if (!class_exists('ThreadCellRestoreDestructor', false)) {
    final class ThreadCellRestoreDestructor
    {
        public static int $runs = 0;
        public static int $lateReleased = 0;

        public function __destruct()
        {
            ++self::$runs;
            echo 'DTOR:' . (string) ($_GET['who'] ?? '-') . ';';
            $late = new ThreadCellRestoreLateProbe();
            register_shutdown_function(static function () use ($late): void {
                echo 'LATE-SHUTDOWN;';
            });
        }
    }

    final class ThreadCellRestoreLateProbe
    {
        public function __destruct()
        {
            ++ThreadCellRestoreDestructor::$lateReleased;
        }
    }
}

$who = (string) ($_GET['who'] ?? '-');

if (($_GET['role'] ?? '') === 'parked') {
    register_shutdown_function(static function () use ($who): void {
        echo "OWN-SHUTDOWN:$who;";
    });
    $sharedState['cell_restore_parked'] = true;
    oxphp_usleep(800_000);
    $sharedState['cell_restore_parked'] = false;
    header('X-After-Resume: 1');
    echo "BODY:$who;";

    return;
}

$probe = new ThreadCellRestoreDestructor();
ob_start(static function (string $buffer) use ($probe): string {
    return $buffer;
}, 0, PHP_OUTPUT_HANDLER_STDFLAGS & ~PHP_OUTPUT_HANDLER_REMOVABLE);
unset($probe);
$sharedState['cell_restore_left'] = true;
echo "BODY:$who;";
