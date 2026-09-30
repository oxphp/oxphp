<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('net_cli_binary_stays_quiet', 'hooks');

// The extension is loaded by the PHP CLI binary as well, and RUNTIME_HOOKS is
// normally set for the whole container, so every php process an image starts —
// composer, artisan, a cron script — goes through the category's startup with the
// variable set. The CLI carries the engine inside its own executable, so there is
// no libphp for the category to intercept and nothing for it to do, which is not
// something to report: a line on stderr from each of those processes reads as a
// fault, and a script that treats any stderr output as a failure would take it for
// one.
$php = '/usr/local/bin/php';
if (!is_executable($php)) {
    $t->assertTrue('the CLI binary is in the image', false);
    $t->done();
    exit;
}

$process = proc_open(
    [$php, '-r', 'echo "ran";'],
    [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
    $pipes
);
$t->assertTrue('the CLI binary started', is_resource($process));

if (is_resource($process)) {
    $stdout = stream_get_contents($pipes[1]);
    $stderr = stream_get_contents($pipes[2]);
    fclose($pipes[1]);
    fclose($pipes[2]);
    $status = proc_close($process);

    $t->assertSame('the script ran', $stdout, 'ran');
    $t->assertSame('and the process ended well', $status, 0);
    $t->assertSame('and said nothing on stderr', trim($stderr), '');
}

$t->done();
