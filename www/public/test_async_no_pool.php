<?php
// Ошибка: async вызвана без пула (ASYNC_WORKERS=0 или не задано)
header('Content-Type: application/json');

try {
    $p = oxphp_async(function(): int { return 1; });
    // Если пул настроен — это просто работает
    $result = oxphp_async_await($p);
    echo json_encode([
        'test'   => 'pool_check',
        'result' => $result,
        'pool'   => 'configured',
        'pass'   => true,
    ]);
} catch (\OxPHP\Async\Exception $e) {
    // Если пул не настроен — OxPHP\Async\Exception
    echo json_encode([
        'test'    => 'pool_check',
        'class'   => get_class($e),
        'message' => $e->getMessage(),
        'pool'    => 'not configured',
        'pass'    => true,
    ]);
}
