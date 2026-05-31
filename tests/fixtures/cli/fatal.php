<?php
// An uncaught Error: the engine sets EG(exit_status) = 255 while reporting the
// fatal, php_execute_script returns normally, and the one-shot propagates that
// 255 (php-cli parity) with the error written to stderr.
echo "before-fatal\n";
this_function_does_not_exist();
echo "after-fatal\n"; // unreachable
