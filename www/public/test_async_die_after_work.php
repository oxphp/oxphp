<?php
// die() после dispatch: промис очищается при RSHUTDOWN,
// ответ отдаётся до die()
header('Content-Type: application/json');

$p1 = oxphp_async(function(): string {
    usleep(100_000); // 100ms работы
    return 'done';
});

// Отправляем ответ клиенту ДО await
echo json_encode([
    'test' => 'die_after_dispatch',
    'note' => 'response sent before die(), promise cleaned up by RSHUTDOWN',
    'pass' => true,
]);

// die() прерывает скрипт — промис $p1 так и не awaited.
// RSHUTDOWN вызовет cleanup_outstanding_promises с 5с таймаутом.
die();
