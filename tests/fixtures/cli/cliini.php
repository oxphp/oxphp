<?php
// The CLI defaults carried in the SAPI ini_entries blob must actually apply:
// max_execution_time=0 (no SIGALRM kill for long runs) and register_argc_argv=1
// (so $_SERVER['argc'] is populated). If the ini_entries pointer is dropped by
// sapi_startup without being re-attached, max_execution_time falls back to the
// engine default (30) and $_SERVER['argc'] disappears.
echo ini_get('max_execution_time'), "|", ($_SERVER['argc'] ?? 'NO-ARGC'), "\n";
