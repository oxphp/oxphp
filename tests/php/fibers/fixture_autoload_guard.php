<?php

declare(strict_types=1);

// Inner request for fibers/test_autoload_guard_after_a_fatal and
// fibers/test_autoload_guard_while_parked. The test arms the class with
// OxphpAutoloadGuard before sending this.
//
// ?role=fail looks the class up, and the autoloader fails with a fatal error —
// in the handler itself with ?case=request, inside a Fiber the handler starts
// with ?case=fiber. With ?check=shutdown a shutdown function then records
// whether the same lookup reaches the autoloader again and finds the class;
// with ?check=next nothing looks it up again, which the test does.
//
// ?role=lookup looks the class up and answers whether it was found.

require_once __DIR__ . '/autoload_guard_probe.php';

ini_set('display_errors', '0');

$class = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['cls'] ?? ''));

if (($_GET['role'] ?? '') === 'lookup') {
    echo json_encode(['exists' => class_exists($class)]);

    return;
}

if (($_GET['check'] ?? '') === 'shutdown') {
    register_shutdown_function(static function () use (&$sharedState, $class): void {
        $sharedState['autoload_guard_shutdown'] = [
            'exists' => class_exists($class),
            'calls' => OxphpAutoloadGuard::calls($class),
        ];
    });
}

if (($_GET['case'] ?? '') === 'fiber') {
    (new \Fiber(static function () use ($class): void {
        class_exists($class);
    }))->start();
} else {
    class_exists($class);
}

echo 'not reached';
