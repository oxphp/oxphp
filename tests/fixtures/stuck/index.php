<?php
// Traditional-mode fixture for tests/stuck_classification.sh. Each route holds
// its worker past the supervisor's stuck threshold in one of the three shapes
// it tells apart, or — `cleanup` — in one the drain deadline then unwinds.
set_time_limit(0);

switch ($_GET['kind'] ?? '') {
    case 'loop': // PHP running, and not a single call inside the loop
        $i = 0;
        for (;;) {
            $i++;
        }
        // unreachable

    case 'sleep': // blocked in the kernel, no CPU
        sleep(3600);
        break;

    case 'ccall': // burning CPU inside one internal call that does not return
        password_hash('x', PASSWORD_BCRYPT, ['cost' => 24]);
        break;

    case 'cleanup': // a loop the drain deadline unwinds, then cleanup past it
        register_shutdown_function(function () {
            // Runs after the unwind, on a request still past the threshold:
            // the supervisor keeps interrupting it, and that must not end it
            // a second time.
            $until = microtime(true) + 1.3;
            while (microtime(true) < $until) {
            }
            error_log('stuck-cleanup: shutdown function finished');
        });
        $i = 0;
        for (;;) {
            $i++;
        }
        // unreachable
}
echo "done\n";
