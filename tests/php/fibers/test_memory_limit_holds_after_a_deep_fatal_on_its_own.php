<?php

declare(strict_types=1);

// Runner line: the deep fatal of fixture_deep_backtrace_fatal.php, run as a
// request of its own — nothing else is in flight on the worker while it runs or
// ends. The line after it checks what it left.

$_GET['step'] = 'fatal';
include __DIR__ . '/fixture_deep_backtrace_fatal.php';
