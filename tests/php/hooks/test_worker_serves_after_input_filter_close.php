<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The request after one that ended holding a filtered php://input handle, whose
// filter's onclose() ran userland from inside the end of that request —
// test_php_input_filter_close_fatal (onclose() fatals) or
// test_php_input_filter_close_drops_handle (onclose() drops the handle's last
// reference). ?case= names which one, so the marker read below is that case's.
//
// Three halves, all needed. The close has to have reached the handle, or the
// request before proved nothing. The worker that ran it has to be the one
// answering this: a worker that lost its serve loop is replaced, and the
// replacement counts this as its first request. And a fatal has to have been
// recovered from to the end rather than merely survived: zend_bailout raises the
// cycle collector's protection beside the unclean-shutdown flag, and outside the
// worker's own recovery only zend_activate lowers it — once per worker, not per
// request. A collector left protected stops collecting for the life of the
// worker, which no single request would notice.

$t = new TestCase('worker_serves_after_input_filter_close', 'hooks');

$case = (string) ($_GET['case'] ?? '');
$t->assertTrue('the suite line names the case', in_array($case, ['fatal', 'drop'], true));

$marker = "/tmp/oxphp-input-filter-close-$case";
$reached = is_file($marker);
$t->assertTrue(
    'the end of the request before closed the filtered php://input handle',
    $reached
);
if ($reached) {
    unlink($marker);
}

$worker = OxPHP\Server\Worker::current();
$t->assertGreaterThan(
    'the worker that ran the close is still serving',
    $worker->requestCount(),
    1
);

if ($case === 'fatal') {
    $t->assertKeyExists(
        'the request before left its handle in worker-scope state',
        $sharedState,
        'php_input_filter_close'
    );
    $t->assertFalse(
        'and the end of that request closed it',
        is_resource($sharedState['php_input_filter_close'] ?? null)
    );
    unset($sharedState['php_input_filter_close']);
} else {
    $t->assertNull(
        'onclose() dropped the only reference to the handle',
        OxPHPInputHandleHolder::$handle
    );
}

$t->assertFalse('the cycle collector is not left protected', gc_status()['protected']);

// A cycle nothing else can reach, so what the collector reports is this one.
$a = new stdClass();
$b = new stdClass();
$a->peer = $b;
$b->peer = $a;
unset($a, $b);

$t->assertGreaterThan('the cycle collector still collects', gc_collect_cycles(), 0);

$t->done();
