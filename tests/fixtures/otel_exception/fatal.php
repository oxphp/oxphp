<?php
// Classless fatal (not a Throwable): the parser synthesizes exception.type from
// the E_* level and records message + file/line, with no stacktrace.
trigger_error('fatal path: kaboom', E_USER_ERROR);
