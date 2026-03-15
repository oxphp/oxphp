<?php
// Frozen-массив: use-переменная замораживается и читается на другом потоке
header('Content-Type: application/json');

$data = [
    'name'    => 'oxphp',
    'numbers' => range(1, 1000),
    'nested'  => ['deep' => ['value' => 42]],
];

$p = oxphp_async(function(int $idx) use ($data): array {
    return [
        'name'   => $data['name'],
        'number' => $data['numbers'][$idx],
        'deep'   => $data['nested']['deep']['value'],
    ];
}, 500);

$result = oxphp_async_await($p);

echo json_encode([
    'test'     => 'frozen_array',
    'result'   => $result,
    'expected' => ['name' => 'oxphp', 'number' => 501, 'deep' => 42],
    'pass'     => $result['name'] === 'oxphp'
              && $result['number'] === 501
              && $result['deep'] === 42,
]);
