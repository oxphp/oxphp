<?php
// Исключение внутри async-замыкания пробрасывается при await
header('Content-Type: application/json');

$p = oxphp_async(function(): void {
    throw new \RuntimeException('something broke');
});

try {
    oxphp_async_await($p);
    echo json_encode(['test' => 'exception_propagation', 'pass' => false, 'error' => 'no exception']);
} catch (\OxPHP\AsyncException $e) {
    echo json_encode([
        'test'    => 'exception_propagation',
        'class'   => get_class($e),
        'message' => $e->getMessage(),
        'pass'    => str_contains($e->getMessage(), 'RuntimeException')
                  && str_contains($e->getMessage(), 'something broke'),
    ]);
}
