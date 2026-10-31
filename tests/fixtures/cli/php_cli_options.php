<?php
// Starts a child through PHP_BINARY with the php command lines that
// PHP_BINARY consumers build (Symfony PhpProcess / PhpSubprocess, PHPUnit
// process isolation and .phpt runner) and prints one line: out=<child stdout>|code=<exit>|err=<first stderr line>,
// or TIMEOUT when the child does not finish within 10 seconds.
$mode = $argv[1] ?? '';

switch ($mode) {
    case 'child-ini':
        echo ini_get('memory_limit'), '|', php_ini_loaded_file(), '|', var_export(php_ini_scanned_files(), true);
        exit(0);
    case 'child-argv':
        echo implode('|', $argv);
        exit(0);
}

function spawn(array $cmd, string $stdin = ''): string
{
    $p = proc_open($cmd, [0 => ['pipe', 'r'], 1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes);
    fwrite($pipes[0], $stdin);
    fclose($pipes[0]);
    stream_set_blocking($pipes[1], false);
    stream_set_blocking($pipes[2], false);
    $out = $err = '';
    $deadline = microtime(true) + 10;
    while (!feof($pipes[1]) || !feof($pipes[2])) {
        $left = $deadline - microtime(true);
        if ($left <= 0) {
            proc_terminate($p, 9);
            proc_close($p);
            return 'TIMEOUT';
        }
        $r = array_filter([$pipes[1], $pipes[2]], fn($s) => !feof($s));
        $w = $e = null;
        if (stream_select($r, $w, $e, 0, (int) ($left * 1e6)) > 0) {
            $out .= stream_get_contents($pipes[1]);
            $err .= stream_get_contents($pipes[2]);
        }
    }
    $code = proc_close($p);
    return 'out=' . trim($out) . '|code=' . $code . '|err=' . strtok(trim($err), "\n");
}

switch ($mode) {
    // Symfony PhpSubprocess: no php.ini, then the merged ini it writes.
    case 'subprocess':
        $ini = tempnam(sys_get_temp_dir(), 'ini');
        file_put_contents($ini, "memory_limit=99M\n");
        echo spawn([PHP_BINARY, '-n', '-c', $ini, __FILE__, 'child-ini']), "|ini=$ini\n";
        break;
    // Symfony PhpProcess and PHPUnit process isolation: the bare binary, the
    // script on standard input.
    case 'stdin':
        echo spawn([PHP_BINARY], "<?php echo 6*7, '|', PHP_SAPI, '|', \$argv[0], '|', __FILE__, '|', \$_SERVER['SCRIPT_FILENAME'];"), "\n";
        break;
    // A script on standard input that is never compiled: the prepend file
    // throws first, and its shutdown function looks at fd 0.
    case 'stdin-prepend':
        echo spawn([PHP_BINARY, '-d', 'auto_prepend_file=' . __DIR__ . '/stdin_prepend_throws.php'], "<?php echo 'main';"), "\n";
        break;
    // PHPUnit process isolation with ini settings to pass on.
    case 'stdin-d':
        echo spawn([PHP_BINARY, '-d', 'memory_limit=88M'], "<?php echo ini_get('memory_limit');"), "\n";
        break;
    case 'r':
        echo spawn([PHP_BINARY, '-r', 'echo 6*7, "|", $argv[0], "|", $argv[1], "|", __FILE__;', 'x']), "\n";
        break;
    case 'r-exit':
        echo spawn([PHP_BINARY, '-r', 'exit(3);']), "\n";
        break;
    case 'r-throw':
        echo spawn([PHP_BINARY, '-r', 'throw new Exception("boom");']), "\n";
        break;
    // A warning from `-r` code names the function that raised it.
    case 'r-warn':
        echo spawn([PHP_BINARY, '-r', 'file_get_contents("/nonexistent");']), "\n";
        break;
    // PHPUnit's .phpt runner for a test with a --STDIN-- section: the code in a
    // file after -f, its arguments right after the file, no `--`.
    case 'f':
        echo spawn([PHP_BINARY, '-f', __FILE__, 'child-argv', 'y']), "\n";
        break;
    // A bare oxphp started in the background through a shell. The shell exits
    // at once, and the delay makes sure the subshell is an orphan before it
    // becomes oxphp, so a run that is PID 1 is its parent.
    case 'bg-serve':
        shell_exec('(sleep 1; LISTEN_ADDR=127.0.0.1:18082 exec ' . escapeshellarg(PHP_BINARY) . ') > /tmp/bg.log 2>&1 &');
        // The subshell is already this process's child before it execs oxphp.
        $adopted = false;
        foreach (glob('/proc/[0-9]*/stat') as $stat) {
            $line = (string) @file_get_contents($stat);
            $fields = explode(' ', substr($line, strrpos($line, ')') + 2));
            $adopted = $adopted || (int) ($fields[1] ?? 0) === getmypid();
        }
        $deadline = microtime(true) + 10;
        do {
            usleep(100000);
            $log = (string) @file_get_contents('/tmp/bg.log');
        } while (!str_contains($log, 'Server listening') && microtime(true) < $deadline);
        echo 'pid=', getmypid(), '|', $adopted ? 'adopted' : 'not-adopted', '|', str_contains($log, 'Server listening') ? 'served' : 'log=' . strtok($log, "\n"), "\n";
        break;
}
