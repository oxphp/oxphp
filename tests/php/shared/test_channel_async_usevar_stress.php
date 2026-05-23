<?php
/**
 * Regression: capturing values (including a Shared\* instance) as closure
 * use-vars in oxphp_async() must deliver them to the worker thread and must
 * not corrupt the PHP heap under sustained concurrent producer/consumer load.
 *
 * Two bugs in the cross-thread closure transfer are exercised here:
 *
 *   1. Value delivery. The dispatcher read the closure's compile-time
 *      static_variables template (IS_UNDEF use-var slots) instead of the
 *      bound HashTable behind the static_variables MAP_PTR slot, so every
 *      use-var arrived as null on the worker. With $ch null, send()/close()
 *      fatal; with $n null the producer loop never ran. Fixed by reading
 *      ZEND_MAP_PTR_GET(static_variables_ptr).
 *
 *   2. Heap corruption. The worker reconstructed the closure from a memcpy'd
 *      op_array without re-initialising static_variables_ptr for its own
 *      thread. The stale offset resolved to a foreign slot, so
 *      zend_create_closure() dup'd a dangling HashTable (use-after-free) that
 *      corrupted the refcount of the captured Shared\* wrapper -> the registry
 *      Entry was freed early -> zend_mm_heap corrupted -> SIGABRT, surfacing
 *      only at volume. Fixed by pointing the slot at the worker's own HT.
 *
 * Passing the channel as an argument sidestepped both (args are not bound
 * into static_variables); use($ch) is required to reach them. got === $n
 * proves $ch and $n were delivered AND no crash occurred. Worker mode only.
 */

header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$ch = new OxPHP\Shared\Channel(1);
$n  = 50000;

$producer = oxphp_async(function () use ($ch, $n): int {
    for ($i = 0; $i < $n; $i++) {
        $ch->send($i);
    }
    $ch->close();
    return $n;
});

$consumer = oxphp_async(function () use ($ch): int {
    $got = 0;
    while (true) {
        $r = $ch->recv();
        if (!$r->isOk()) {
            break;
        }
        $got++;
    }
    return $got;
});

oxphp_async_await($producer);
$got = oxphp_async_await($consumer);

if ($got !== $n) {
    echo "FAIL: expected $n received, got $got\n";
    return;
}

echo "OK\n";
