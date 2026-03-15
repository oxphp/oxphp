<?php
// Цепочка исключений: DomainException → AsyncException
header('Content-Type: application/json');

$p = oxphp_async(function(): never {
    throw new \DomainException('invalid domain value', 422);
});

try {
    oxphp_async_await($p);
    echo json_encode(['test' => 'nested_exception', 'pass' => false]);
} catch (\OxPHP\AsyncException $e) {
    echo json_encode([
        'test'       => 'nested_exception',
        'class'      => get_class($e),
        'message'    => $e->getMessage(),
        'contains'   => [
            'DomainException' => str_contains($e->getMessage(), 'DomainException'),
            'message'         => str_contains($e->getMessage(), 'invalid domain value'),
        ],
        'pass'       => str_contains($e->getMessage(), 'DomainException')
                     && str_contains($e->getMessage(), 'invalid domain value'),
    ]);
}
