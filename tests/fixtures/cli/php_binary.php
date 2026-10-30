<?php
// Starts a child through PHP_BINARY with the command line amphp/parallel
// builds: -d flags, then a script path and its arguments.
//   line 1: parent|PHP_BINARY|pid
//   line 2: child|PHP_SAPI|PHP_BINARY|memory_limit|pid
if (($argv[1] ?? '') === 'child') {
    echo 'child|', PHP_SAPI, '|', PHP_BINARY, '|', ini_get('memory_limit'), '|', getmypid(), "\n";
    exit(0);
}
echo 'parent|', PHP_BINARY, '|', getmypid(), "\n";
passthru(escapeshellarg(PHP_BINARY) . ' -dmemory_limit=77M ' . escapeshellarg(__FILE__) . ' child 2>&1');
