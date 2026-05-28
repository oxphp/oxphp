<?php
/**
 * Regression (in-transit lifetime): a Shared\* RETURNED from an oxphp_async()
 * closure stays alive in transit. The closure creates the value and returns it,
 * dropping its only reference at frame teardown; the returned value's keepalive
 * must ride the async result so the entry survives until the awaiting fiber
 * materializes it. Without it, await() re-resolves a freed id and yields NULL
 * (silent data loss) — the async-pool sibling of the Channel in-transit case.
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$p = oxphp_async(function () {
    $c = new OxPHP\Shared\Counter();
    $c->add(7);
    return $c; // serialized to tag-7; $c dropped at frame teardown
});

$c = oxphp_async_await($p);

if (!($c instanceof OxPHP\Shared\Counter) || $c->get() !== 7) {
    echo 'FAIL: counter via async return: ', var_export($c, true), "\n";
    return;
}

echo "OK\n";
