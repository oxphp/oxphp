<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

use OxPHP\Shared\Pool;

// Code eval() runs takes the class of the frame that runs it, so self::$s there
// is the static property of the class the method belongs to. For a class its
// request links in its own memory, that is the copy of the request that created
// the pool, and a string a task appended to it, in the task thread's memory,
// would be freed by this thread ("zend_mm_heap corrupted"). The callback is
// refused there, though nothing in the method's own code names self::.
$t = new TestCase(
    'test_pool_callback_appending_through_eval_to_a_class_linked_in_request_memory_is_refused_on_another_thread',
    'asyncdecl',
);

asyncdecl_load_linked_classes();

AsyncDeclLinkedMaker::$s = str_repeat('y', 40);
$pool = new Pool(factory: [AsyncDeclLinkedMaker::class, 'appendsViaEval'], destroy: null, maxSize: 1);
$t->assertSame(
    'refused in a task',
    asyncdecl_acquire_in_task($pool, 'v'),
    [\Error::class, 'Class ' . AsyncDeclLinkedMaker::class . ASYNCDECL_LINKED_TAIL],
);
$t->assertSame('the task did not append', strlen(AsyncDeclLinkedMaker::$s), 40);

$h = $pool->acquire();
$t->assertSame('on this thread the factory appends', $h->get()->v, 5040);
$h->release();
unset($h, $pool);
$t->assertSame('this thread keeps what it appended', strlen(AsyncDeclLinkedMaker::$s), 5040);

$t->done();
