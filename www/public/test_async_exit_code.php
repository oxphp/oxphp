<?php
// exit(int) внутри async — тоже ловится как исключение
header('Content-Type: application/json');

$p = oxphp_async(function(): never {
    exit(42);
});

try {
    oxphp_async_await($p);
    echo json_encode(['test' => 'exit_code_in_closure', 'pass' => false]);
} catch (\OxPHP\AsyncException $e) {
    echo json_encode([
        'test'    => 'exit_code_in_closure',
        'class'   => get_class($e),
        'message' => $e->getMessage(),
        'pass'    => true,
    ]);
}
