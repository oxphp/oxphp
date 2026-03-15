<?php
// Несколько die() подряд: убеждаемся что async worker pool не ломается.
// Каждый запрос dispatch'ит задачу с die(), await ловит исключение,
// и следующий dispatch работает нормально.
header('Content-Type: application/json');

$tests = [];

// Раунд 1: die()
$p = oxphp_async(function(): never { die('round1'); });
try {
    oxphp_async_await($p);
    $tests[] = ['round' => 1, 'pass' => false];
} catch (\OxPHP\AsyncException $e) {
    $tests[] = ['round' => 1, 'pass' => true, 'msg' => $e->getMessage()];
}

// Раунд 2: нормальная задача — пул не мёртв
$p = oxphp_async(function(): int { return 42; });
try {
    $r = oxphp_async_await($p);
    $tests[] = ['round' => 2, 'pass' => $r === 42, 'result' => $r];
} catch (\Throwable $e) {
    $tests[] = ['round' => 2, 'pass' => false, 'error' => $e->getMessage()];
}

// Раунд 3: exit(1)
$p = oxphp_async(function(): never { exit(1); });
try {
    oxphp_async_await($p);
    $tests[] = ['round' => 3, 'pass' => false];
} catch (\OxPHP\AsyncException $e) {
    $tests[] = ['round' => 3, 'pass' => true, 'msg' => $e->getMessage()];
}

// Раунд 4: опять нормальная задача
$p = oxphp_async(function(): string { return 'alive'; });
try {
    $r = oxphp_async_await($p);
    $tests[] = ['round' => 4, 'pass' => $r === 'alive', 'result' => $r];
} catch (\Throwable $e) {
    $tests[] = ['round' => 4, 'pass' => false, 'error' => $e->getMessage()];
}

$all_pass = count(array_filter($tests, fn($t) => $t['pass'])) === count($tests);

echo json_encode([
    'test'  => 'die_sequence_recovery',
    'tests' => $tests,
    'pass'  => $all_pass,
    'note'  => 'async pool survives die()/exit() in closures',
]);
