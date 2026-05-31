<?php
// Exercises the async engine under the one-shot CLI role: oxphp_sleep must
// be registered and suspend for roughly the requested duration.
if (!function_exists('oxphp_sleep')) {
    fwrite(STDERR, "oxphp_sleep not registered\n");
    exit(1);
}
$t0 = microtime(true);
oxphp_sleep(0.02);
$elapsed = microtime(true) - $t0;
echo ($elapsed >= 0.015) ? "async-ok\n" : "async-too-fast\n";
