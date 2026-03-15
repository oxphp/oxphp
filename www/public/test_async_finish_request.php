<?php
// oxphp_finish_request() + async: отправляем ответ рано,
// затем продолжаем фоновую работу через async/await
header('Content-Type: application/json');

// Dispatch фоновую задачу
$p = oxphp_async(function(): string {
    usleep(100_000); // 100ms фоновой работы
    return 'background_done';
});

// Отправляем HTTP-ответ клиенту сразу
echo json_encode([
    'test'    => 'finish_request_then_async',
    'status'  => 'response sent early',
    'pass'    => true,
]);
oxphp_finish_request();

// Клиент уже получил ответ, но мы можем await результат
$result = oxphp_async_await($p);
// $result === 'background_done', но клиент этого не увидит —
// вывод после finish_request уходит в /dev/null
