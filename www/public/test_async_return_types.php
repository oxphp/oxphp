<?php
// Разные типы возврата: null, bool, int, float, string, array
header('Content-Type: application/json');

$tests = [];

// null
$p = oxphp_async(function(): mixed { return null; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'null',   'pass' => $r === null];

// true
$p = oxphp_async(function(): bool { return true; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'true',   'pass' => $r === true];

// false
$p = oxphp_async(function(): bool { return false; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'false',  'pass' => $r === false];

// int
$p = oxphp_async(function(): int { return PHP_INT_MAX; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'int',    'pass' => $r === PHP_INT_MAX];

// float
$p = oxphp_async(function(): float { return M_PI; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'float',  'pass' => abs($r - M_PI) < 1e-10];

// string
$p = oxphp_async(function(): string { return "Привет, мир! 🌍"; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'string', 'pass' => $r === "Привет, мир! 🌍"];

// array (nested)
$p = oxphp_async(function(): array {
    return ['a' => 1, 'b' => [2, 3], 'c' => ['d' => ['e' => 4]]];
});
$r = oxphp_async_await($p);
$tests[] = ['type' => 'array',  'pass' => $r['c']['d']['e'] === 4 && $r['b'] === [2, 3]];

// empty string
$p = oxphp_async(function(): string { return ''; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'empty_string', 'pass' => $r === ''];

// zero
$p = oxphp_async(function(): int { return 0; });
$r = oxphp_async_await($p);
$tests[] = ['type' => 'zero', 'pass' => $r === 0];

$all_pass = count(array_filter($tests, fn($t) => $t['pass'])) === count($tests);

echo json_encode([
    'test'  => 'return_types',
    'tests' => $tests,
    'pass'  => $all_pass,
]);
