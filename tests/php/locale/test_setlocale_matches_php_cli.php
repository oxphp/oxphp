<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// setlocale() inside a request answers what the PHP CLI answers for the same
// calls, step by step: the name each call returns, the LC_ALL name after it,
// and which calls fail. setlocale_sequence.php holds the calls; the CLI runs
// them with no ini file, so it is PHP's own setlocale() on the process locale.
$t = new TestCase('test_setlocale_matches_php_cli', 'locale');

$php = '/usr/local/bin/php';
$sequence = __DIR__ . '/setlocale_sequence.php';
if (!is_executable($php)) {
    $t->assertTrue('the CLI binary is in the image', false);
    $t->done();
    exit;
}

$startHere = setlocale(LC_ALL, '0');
$here = (require $sequence)();

$process = proc_open(
    [$php, '-n', '-r', 'echo json_encode([setlocale(LC_ALL, "0"), (require $argv[1])()]);', $sequence],
    [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
    $pipes
);
$t->assertTrue('the CLI binary started', is_resource($process));
if (!is_resource($process)) {
    $t->done();
    exit;
}
$stdout = stream_get_contents($pipes[1]);
$stderr = stream_get_contents($pipes[2]);
fclose($pipes[1]);
fclose($pipes[2]);
$status = proc_close($process);

$t->assertSame('the CLI run ended well', $status, 0);
$t->assertSame('and said nothing on stderr', trim((string) $stderr), '');
$decoded = json_decode((string) $stdout, true);
$t->assertTrue('and answered both results', is_array($decoded) && count($decoded) === 2);
[$startCli, $cli] = is_array($decoded) && count($decoded) === 2 ? $decoded : [null, []];

$t->assertSame('both start from the same locale', $startHere, $startCli);
$t->assertSame('both ran every step', count($here), count($cli));
foreach ($here as $i => $step) {
    $t->assertSame("step $i returns what the CLI returns", $step['ret'], $cli[$i]['ret'] ?? 'missing');
    $t->assertSame("and leaves the LC_ALL name the CLI has", $step['all'], $cli[$i]['all'] ?? 'missing');
}

// Not a premise of the comparison, only of it reaching the cases it is for: a
// step whose name every libc refuses compares nothing but two falses.
$t->assertTrue(
    'the series reaches LC_ALL names whose categories differ',
    count(array_filter($here, static fn (array $s): bool => is_string($s['ret']) && str_contains($s['ret'], ';'))) >= 2
);

$t->done();
