<?php
// stdout line, then a warning which must land on stderr (display_errors=stderr).
echo "on-stdout\n";
fwrite(STDERR, "on-stderr\n");
trigger_error("a notice", E_USER_WARNING);
