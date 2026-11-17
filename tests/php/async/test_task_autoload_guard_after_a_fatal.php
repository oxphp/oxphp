<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A fatal inside an autoloader in an oxphp_async() task does not leave the
// class marked as being loaded on the async worker. The engine adds the name to
// a table before it calls the autoloaders and takes it out after they return; a
// lookup that finds the name there answers "not found" without calling them. A
// fatal skips the taking out, and the async worker's thread lives on, so left
// alone the name stays: every later task on that thread is told the class does
// not exist without its autoloader being called.
//
// This profile has one async worker, so the next task runs on the thread the
// fatal left.

$t = new TestCase('task_autoload_guard_after_a_fatal', 'async');
$class = 'OxphpTaskAutoloadFatal' . bin2hex(random_bytes(4));

$failed = oxphp_async(static function (string $class): void {
    // Autoloaders outlive the task on its thread, and the fatal skips any
    // unregistering: this one fails once and is a no-op after.
    $armed = true;
    spl_autoload_register(static function (string $name) use ($class, &$armed): void {
        if (!$armed || strcasecmp($name, $class) !== 0) {
            return;
        }
        $armed = false;
        // The name is made of letters and digits only.
        eval("abstract class {$class}Broken { abstract public function f() {} }");
    });
    class_exists($class);
}, $class);
$failure = '';
try {
    oxphp_async_await($failed);
} catch (\Throwable $e) {
    $failure = $e->getMessage();
}
$t->assertContains('the task failed in its autoloader', $failure, 'cannot contain body');

$next = oxphp_async_await(oxphp_async(static function (string $class): array {
    $calls = 0;
    $loader = static function (string $name) use ($class, &$calls): void {
        if (strcasecmp($name, $class) === 0) {
            $calls++;
            eval("final class $class {}");
        }
    };
    spl_autoload_register($loader);
    try {
        $exists = class_exists($class);
    } finally {
        spl_autoload_unregister($loader);
    }

    return ['exists' => $exists, 'calls' => $calls];
}, $class));
$t->meta('next', $next);
$t->assertSame('a lookup in the next task on the thread reaches the autoloader', $next['calls'] ?? null, 1);
$t->assertTrue('and finds the class', $next['exists'] ?? null);

$t->done();
