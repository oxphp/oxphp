<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;
use OxPHP\Shared\SharedException;

// Outside worker mode a pool lives as long as the request that creates it, and
// a registry name lives as long as the server. Neither registry method binds a
// pool to a name: Registry::pool() refuses before its factory runs, and
// Registry::global() refuses a pool its factory returns before the name is
// taken. Other Shared types still bind, and a pool made with `new` still
// serves the request that made it.
$t = new TestCase('test_registry_refuses_pool', 'pooltrad');

$run = bin2hex(random_bytes(6));
$calls = 0;
$factory = static function () use (&$calls): Pool {
    $calls++;
    return new Pool(factory: static fn (): \ArrayObject => new \ArrayObject(), destroy: null, maxSize: 1);
};
$attempt = static function (callable $bind): array {
    try {
        $bind();
        return [null, ''];
    } catch (\Throwable $e) {
        return [get_class($e), $e->getMessage()];
    }
};

[$threw, $message] = $attempt(static fn () => Registry::pool("pooltrad-named-$run", $factory));
$t->assertSame('Registry::pool() is refused', $threw, SharedException::class);
$t->assertContains('the refusal names the method', $message, 'Registry::pool() cannot be called from a request served without worker mode');
$t->assertSame('the factory never ran', $calls, 0);
$t->assertFalse('the name stays free', in_array("pooltrad-named-$run", Registry::keys(), true));

[$threw, $message] = $attempt(static fn () => Registry::global("pooltrad-global-$run", $factory));
$t->assertSame('Registry::global() returning a pool is refused', $threw, SharedException::class);
$t->assertContains('that refusal names the method', $message, 'Registry::global() cannot bind a Shared\\Pool to a name in a request served without worker mode');
$t->assertSame('its factory ran once', $calls, 1);
$t->assertFalse('that name stays free too', in_array("pooltrad-global-$run", Registry::keys(), true));
$after = Registry::global("pooltrad-global-$run", static fn (): Map => new Map());
$t->assertInstanceOf('the refused name can be bound to another type', $after, Map::class);

$map = Registry::map("pooltrad-map-$run", static fn (): Map => new Map());
$t->assertInstanceOf('Registry::map() still binds', $map, Map::class);

$pool = new Pool(factory: static fn (): \ArrayObject => new \ArrayObject(['marker' => 'here']), destroy: null, maxSize: 1);
$h = $pool->acquire();
$t->assertSame('a pool made in this request serves it', $h->get()['marker'], 'here');
$h->release();

$t->done();
