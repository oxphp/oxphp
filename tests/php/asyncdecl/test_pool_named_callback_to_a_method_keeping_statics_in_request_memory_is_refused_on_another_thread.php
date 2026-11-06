<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Pool;

// A method of an enum opcache.preload did not load, or of a class whose parent,
// interface or trait is not in OPcache's shared memory, is linked by each
// request that loads it, and keeps its `static` variables in that request's
// memory whichever thread calls it. Named as a Shared\Pool callback, it is
// refused on a thread other than the one that created the pool: a string that
// thread appended to one of them, in its own memory, would be freed by this one
// ("zend_mm_heap corrupted").
$t = new TestCase(
    'test_pool_named_callback_to_a_method_keeping_statics_in_request_memory_is_refused_on_another_thread',
    'asyncdecl',
);

enum AsyncDeclLinkedAppendSuit
{
    case Hearts;

    public static function appended(): object
    {
        static $log = '';
        $log .= str_repeat('x', 3);
        return (object) ['v' => $log];
    }
}

asyncdecl_load_linked_classes();

foreach ([AsyncDeclLinkedAppendSuit::class, AsyncDeclLinkedMaker::class] as $class) {
    $refused = [\Error::class, "Method $class::appended()" . ASYNCDECL_STATIC_TAIL];

    $class::appended();
    foreach ([[$class, 'appended'], "$class::appended"] as $i => $factory) {
        $form = $i === 0 ? 'an array' : 'a string';
        $pool = new Pool(factory: $factory, destroy: null, maxSize: 1);
        $t->assertSame("$class, named by $form: refused in a task", asyncdecl_acquire_in_task($pool), $refused);
        $t->assertSame("$class, named by $form: refused again in a task", asyncdecl_acquire_in_task($pool), $refused);
    }

    $pool = new Pool(factory: [$class, 'appended'], destroy: null, maxSize: 1);
    $h = $pool->acquire();
    $t->assertSame(
        "$class: on this thread the factory appends to the method's",
        $h->get()->v,
        'xxxxxx',
    );
    $h->release();
    unset($h, $pool);
    $t->assertSame("$class: the method appends to the factory's", $class::appended()->v, 'xxxxxxxxx');
}

$t->done();
