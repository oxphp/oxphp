<?php
// Промис не awaited: RSHUTDOWN подчищает, сервер не течёт.
// Этот тест проверяет, что скрипт завершается нормально.
header('Content-Type: application/json');

// Dispatch задачу, но не делаем await
$p1 = oxphp_async(function(): string {
    usleep(50_000);
    return 'orphan1';
});

$p2 = oxphp_async(function(): string {
    usleep(50_000);
    return 'orphan2';
});

// Только одну awaim'им
$result = oxphp_async_await($p1);

echo json_encode([
    'test'        => 'not_awaited_cleanup',
    'awaited'     => $result,
    'not_awaited' => 'p2 will be cleaned up by RSHUTDOWN',
    'pass'        => $result === 'orphan1',
]);

// $p2 так и не awaited — RSHUTDOWN cleanup подхватит его
