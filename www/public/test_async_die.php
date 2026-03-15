<?php
// die() / exit() внутри async-замыкания: не убивает сервер,
// а пробрасывается как AsyncException при await
header('Content-Type: application/json');

$p = oxphp_async(function(): void {
    die('goodbye');
});

try {
    oxphp_async_await($p);
    echo json_encode(['test' => 'die_in_closure', 'pass' => false, 'error' => 'no exception']);
} catch (\OxPHP\AsyncException $e) {
    echo json_encode([
        'test'    => 'die_in_closure',
        'class'   => get_class($e),
        'message' => $e->getMessage(),
        'pass'    => true,
        'note'    => 'die() caught safely, server alive',
    ]);
}
