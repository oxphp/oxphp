<?php
// oxphp_async_await_all: собираем результаты нескольких промисов
header('Content-Type: application/json');

$promises = [];
for ($i = 0; $i < 5; $i++) {
    $promises[] = oxphp_async(function(int $n): int {
        return $n * $n;
    }, $i);
}

$results = oxphp_async_await_all($promises);

// Результат — ассоциативный массив [promise_id => value]
$values = array_values($results);
sort($values);

echo json_encode([
    'test'     => 'await_all',
    'values'   => $values,
    'expected' => [0, 1, 4, 9, 16],
    'pass'     => $values === [0, 1, 4, 9, 16],
]);
