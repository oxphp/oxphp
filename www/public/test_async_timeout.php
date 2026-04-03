<?php
// Таймаут ожидания: async-задача спит 5с, а мы ждём только 0.1с
header('Content-Type: application/json');

$p = oxphp_async(function(): string {
    sleep(5);
    return 'too late';
});

try {
    oxphp_async_await($p, 0.1); // 100ms таймаут
    echo json_encode(['test' => 'timeout', 'pass' => false, 'error' => 'no timeout']);
} catch (\OxPHP\Async\TimeoutException $e) {
    echo json_encode([
        'test'    => 'timeout',
        'class'   => get_class($e),
        'message' => $e->getMessage(),
        'pass'    => true,
    ]);
} catch (\OxPHP\Async\Exception $e) {
    echo json_encode([
        'test'    => 'timeout',
        'class'   => get_class($e),
        'message' => $e->getMessage(),
        'pass'    => false,
        'note'    => 'expected OxPHP\\Async\\TimeoutException, got OxPHP\\Async\\Exception',
    ]);
}
