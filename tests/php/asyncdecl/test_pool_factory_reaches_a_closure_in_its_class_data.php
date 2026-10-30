<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// PHP 8.5 accepts a static closure in a class constant and in a property's
// default value. A factory that reads a constant or a static property through
// self:: or static:: has the closures in the constant expressions of the class
// it stands for checked, and a factory that creates an object through
// new self() has its class checked whole, those closures included.
$t = new TestCase('test_pool_factory_reaches_a_closure_in_its_class_data', 'asyncdecl');

if (PHP_VERSION_ID < 80500) {
    $t->meta('php', 'PHP 8.4 has no closures in constant expressions');
    $t->assertTrue('nothing to check before PHP 8.5', true);
    $t->done();
}

require __DIR__ . '/fixture_class_data_closures.php';
$fixture = __DIR__ . '/fixture_class_data_closures.php';

$cases = [
    'constant' => [\AsyncDeclClassDataConstant::class, 'make'],
    'static property' => [\AsyncDeclClassDataStatic::class, 'make'],
    'property' => [\AsyncDeclClassDataProperty::class, 'make'],
    "a child's static property" => [\AsyncDeclClassDataChild::class, 'make'],
];
foreach ($cases as $what => $factory) {
    $pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame(
        "a closure in a $what: in a task",
        asyncdecl_acquire_in_task($pool),
        asyncdecl_refused('asyncdecl_class_data_helper', $fixture, ASYNCDECL_CLASS_DATA_LINES[$what]),
    );
    $h = $pool->acquire();
    $t->assertSame("a closure in a $what: on the worker thread the factory runs", $h->get()->v, 1);
    $h->release();
}

$t->done();
