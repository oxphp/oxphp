<?php
// Невалидные аргументы: ресурсы и объекты запрещены как аргументы
header('Content-Type: application/json');

$tests = [];

// 1. Ресурс как аргумент — должен бросить AsyncException
try {
    $fp = fopen('php://memory', 'r');
    oxphp_async(function($r) { return 1; }, $fp);
    fclose($fp);
    $tests[] = ['name' => 'resource_arg', 'pass' => false, 'error' => 'no exception'];
} catch (\OxPHP\AsyncException $e) {
    if (isset($fp) && is_resource($fp)) fclose($fp);
    $tests[] = ['name' => 'resource_arg', 'pass' => true, 'message' => $e->getMessage()];
}

// 2. Объект как аргумент — должен бросить AsyncException
try {
    $obj = new \stdClass();
    oxphp_async(function($o) { return 1; }, $obj);
    $tests[] = ['name' => 'object_arg', 'pass' => false, 'error' => 'no exception'];
} catch (\OxPHP\AsyncException $e) {
    $tests[] = ['name' => 'object_arg', 'pass' => true, 'message' => $e->getMessage()];
}

// 3. Скалярные аргументы — должны работать
try {
    $p = oxphp_async(function(int $a, float $b, string $c, bool $d): string {
        return "$a,$b,$c,$d";
    }, 42, 3.14, 'hello', true);
    $result = oxphp_async_await($p);
    $tests[] = ['name' => 'scalar_args', 'pass' => $result === '42,3.14,hello,1', 'result' => $result];
} catch (\Throwable $e) {
    $tests[] = ['name' => 'scalar_args', 'pass' => false, 'error' => $e->getMessage()];
}

// 4. Нулевые аргументы — пустая лямбда
try {
    $p = oxphp_async(function(): string { return 'zero'; });
    $result = oxphp_async_await($p);
    $tests[] = ['name' => 'zero_args', 'pass' => $result === 'zero'];
} catch (\Throwable $e) {
    $tests[] = ['name' => 'zero_args', 'pass' => false, 'error' => $e->getMessage()];
}

$all_pass = count(array_filter($tests, fn($t) => $t['pass'])) === count($tests);

echo json_encode([
    'test'  => 'argument_validation',
    'tests' => $tests,
    'pass'  => $all_pass,
]);
