<?php
// Базовый тест: dispatch + await скалярного результата
header('Content-Type: application/json');

$p = oxphp_async(function(int $x, int $y): int {
    return $x + $y;
}, 10, 20);

$result = oxphp_async_await($p);

echo json_encode([
    'test'     => 'basic_dispatch_await',
    'result'   => $result,
    'expected' => 30,
    'pass'     => $result === 30,
]);
