<?php

declare(strict_types=1);

// Inner request for fibers/test_userland_fiber_keeps_its_call_cells.
//
// ?step=suspend starts a fiber that suspends inside unserialize(), in the error
// handler for the stray byte after a whole array holding an object of its own,
// and leaves it suspended for the next request.
//
// ?step=resume resumes that fiber, which finishes its call, and then probes
// unserialize() on its own.
//
// ?step=sort resumes the fiber the test left suspended in the comparator of its
// sort, while the test is parked reading this request.

require_once __DIR__ . '/thread_cell_probe.php';

ini_set('display_errors', '0');

$step = (string) ($_GET['step'] ?? '');

if ($step === 'suspend') {
    $own = new stdClass();
    $own->who = 'fiber';
    set_error_handler(static function (): bool {
        Fiber::suspend();

        return true;
    });
    ThreadCellFiberHolder::$unserialize = new Fiber(static fn () => unserialize(serialize([$own]) . 'X'));
    ThreadCellFiberHolder::$unserialize->start();
    restore_error_handler();
    echo json_encode(['suspended' => ThreadCellFiberHolder::$unserialize->isSuspended()]);

    return;
}

if ($step === 'resume') {
    $fiber = ThreadCellFiberHolder::$unserialize;
    ThreadCellFiberHolder::$unserialize = null;
    $fiber?->resume();
    $result = $fiber !== null && $fiber->isTerminated() ? $fiber->getReturn() : null;
    echo json_encode([
        'result_who' => is_array($result) && isset($result[0]) && is_object($result[0]) ? ($result[0]->who ?? null) : null,
        'after' => thread_cell_unserialize_probe(),
    ]);

    return;
}

if ($step === 'sort') {
    $fiber = ThreadCellFiberHolder::$sort;
    ThreadCellFiberHolder::$sort = null;
    $sorted = null;
    $error = null;
    try {
        $fiber?->resume();
        $sorted = $fiber !== null && $fiber->isTerminated() ? $fiber->getReturn() : null;
    } catch (\Throwable $e) {
        $error = get_class($e) . ': ' . $e->getMessage();
    }
    echo json_encode(['sorted' => $sorted, 'error' => $error]);
}
