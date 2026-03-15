<?php
// Строковые данные: frozen-строки через use, deep-copy строк через аргументы
header('Content-Type: application/json');

$prefix = 'Hello';
$suffix = str_repeat('!', 1000); // длинная строка

$p = oxphp_async(function(string $name) use ($prefix, $suffix): string {
    return $prefix . ', ' . $name . $suffix;
}, 'World');

$result = oxphp_async_await($p);

$expected = $prefix . ', World' . $suffix;

echo json_encode([
    'test'   => 'string_transfer',
    'len'    => strlen($result),
    'pass'   => $result === $expected,
]);
