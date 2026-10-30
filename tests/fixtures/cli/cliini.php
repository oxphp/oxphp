<?php
// The CLI defaults carried in the SAPI ini_entries blob must actually apply:
// max_execution_time=0 (no SIGALRM kill for long runs). If the ini_entries
// pointer is dropped by sapi_startup without being re-attached, it falls back to
// the engine default (30). $_SERVER['argc'] does not show the blob: PHP 8.5
// always sets it, and 8.4 does without the blob unless a php.ini turns
// register_argc_argv off.
echo ini_get('max_execution_time'), "|", ($_SERVER['argc'] ?? 'NO-ARGC'), "\n";
