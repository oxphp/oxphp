<?php

declare(strict_types=1);

// Runner line: the deep fatal of fixture_deep_backtrace_fatal.php, whose
// shutdown function then keeps 200 MiB in the worker's shared state while the
// fatal's backtrace still keeps memory_limit from being enforced. The line after
// it checks what became of the worker.

$_GET['step'] = 'stash';
include __DIR__ . '/fixture_deep_backtrace_fatal.php';
