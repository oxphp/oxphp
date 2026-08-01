<?php

/**
 * Minimal PSR-4 autoloader for the vendored revolt/event-loop v1.0.9 in src/.
 *
 * The library is pinned here rather than installed with composer so the test
 * profile has no network dependency and the behaviour under test is tied to one
 * known revision: the collision these tests describe lives in
 * AbstractDriver::getSuspension() and FiberLocal, and both key on
 * \Fiber::getCurrent().
 */

declare(strict_types=1);

spl_autoload_register(static function (string $class): void {
    if (!str_starts_with($class, 'Revolt\\')) {
        return;
    }

    $relative = str_replace('\\', '/', substr($class, strlen('Revolt\\')));
    $file = __DIR__ . '/src/' . $relative . '.php';

    if (is_file($file)) {
        require $file;
    }
});
