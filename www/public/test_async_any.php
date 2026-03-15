<?php
// oxphp_async_await_any: возвращает первый завершившийся результат
header('Content-Type: application/json');

$promises = [];
for ($i = 1; $i <= 3; $i++) {
    $promises[] = oxphp_async(function(int $n): int {
        return $n * 10;
    }, $i);
}

$result = oxphp_async_await_any($promises);

// Возвращает ['id' => int, 'value' => mixed]
$valid = is_array($result)
      && isset($result['id'], $result['value'])
      && in_array($result['value'], [10, 20, 30], true);

echo json_encode([
    'test'  => 'await_any',
    'result' => $result,
    'pass'  => $valid,
]);
