<?php
/**
 * Pool — factory returns non-object.
 *
 * v1 requires the factory to return an object (identity is stored in
 * the Handle via Z_OBJ). Scalars / arrays must surface as
 * Shared\TypeException. Budget is refunded so the pool stays healthy.
 */

header('Content-Type: text/plain');

$scenarios = [
    'int'    => fn(): int    => 42,
    'string' => fn(): string => 'nope',
    'array'  => fn(): array  => ['x'],
    'null'   => fn(): mixed  => null,
];

foreach ($scenarios as $label => $factory) {
    $pool = new OxPHP\Shared\Pool($factory, null, 1);

    $caught = null;
    try {
        $pool->acquire();
    } catch (\OxPHP\Shared\TypeException $e) {
        $caught = $e;
    }

    if ($caught === null) {
        echo "FAIL[$label]: non-object factory must throw TypeException\n"; exit;
    }
    if ($pool->count() !== 0) {
        echo "FAIL[$label]: size must refund to 0, got " . $pool->count() . "\n"; exit;
    }
    if ($pool->inUse() !== 0) {
        echo "FAIL[$label]: inUse must refund to 0\n"; exit;
    }
}

echo "OK\n";
