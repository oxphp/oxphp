<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// SHARED_ENABLED=false, end to end: the half of the contract that lives in
// PHP's class table. The Rust unit tests prove the plugin hands the host no
// class descriptors; only a running server proves the host therefore builds
// no classes — the step between the two is the class-table population that
// no unit test executes.
//
// Every name below is one the plugin registers when the switch is on, so a
// gate that leaked even one family would show up here.
$t = new TestCase('classes_absent', 'shared_off');

// `false` for the autoload argument throughout: an autoloader that invented
// one of these names would turn a real absence into a green assertion, and a
// `Shared\*` polyfill is exactly the thing an operator might ship after
// switching the subsystem off.
$classes = [
    'OxPHP\\Shared\\Counter',
    'OxPHP\\Shared\\Atomic',
    'OxPHP\\Shared\\Flag',
    'OxPHP\\Shared\\Once',
    'OxPHP\\Shared\\Mutex',
    'OxPHP\\Shared\\Channel',
    'OxPHP\\Shared\\Map',
    'OxPHP\\Shared\\Pool',
    'OxPHP\\Shared\\Registry',
    'OxPHP\\Shared\\Pool\\Handle',
    'OxPHP\\Shared\\Pool\\Stats',
    'OxPHP\\Shared\\Map\\KeyCursor',
    'OxPHP\\Shared\\Channel\\RecvResult',
    'OxPHP\\Shared\\Channel\\SendResult',
];
foreach ($classes as $class) {
    $t->assertFalse("class $class is not registered", class_exists($class, false));
}

// The exception hierarchy goes with them. It is registered by its own call,
// ahead of the type classes, so it is the first thing a gate placed one line
// too low would leave behind.
$exceptions = [
    'OxPHP\\Shared\\SharedException',
    'OxPHP\\Shared\\CapacityException',
    'OxPHP\\Shared\\ClosedException',
    'OxPHP\\Shared\\ContentionException',
    'OxPHP\\Shared\\CorruptedMutexException',
    'OxPHP\\Shared\\CycleException',
    'OxPHP\\Shared\\DeadlockException',
    'OxPHP\\Shared\\InvalidOrderingException',
    'OxPHP\\Shared\\OperationTimeoutException',
    'OxPHP\\Shared\\PoisonedException',
    'OxPHP\\Shared\\StaleHandleException',
    'OxPHP\\Shared\\TypeException',
    'OxPHP\\Shared\\UninitializedException',
    'OxPHP\\Shared\\ValueTooLargeException',
];
foreach ($exceptions as $class) {
    $t->assertFalse("exception $class is not registered", class_exists($class, false));
}

// Enums are a separate registration call from classes, and `Ordering` is
// registered before every type that names it — another distinct point a
// partial gate could stop at.
$enums = [
    'OxPHP\\Shared\\Ordering',
    'OxPHP\\Shared\\Once\\Status',
    'OxPHP\\Shared\\Once\\FailureMode',
    'OxPHP\\Shared\\Channel\\RecvStatus',
    'OxPHP\\Shared\\Channel\\SendStatus',
];
foreach ($enums as $enum) {
    $t->assertFalse("enum $enum is not registered", enum_exists($enum, false));
}

// Free functions the plugin registers alongside the classes.
foreach ([
    'oxphp_pool_spike_capture',
    'oxphp_pool_spike_invoke',
    'oxphp_pool_spike_reset',
] as $fn) {
    $t->assertFalse("function $fn() is not registered", function_exists($fn));
}

// The sentence the documentation puts in front of operators, pinned verbatim.
// `class_exists` returning false already proves the name is gone; this proves
// what PHP *does* about it, which is what an operator actually meets: an
// `Error` (not a warning, not a fatal that bypasses `catch`) carrying this
// exact text.
$err = null;
try {
    new OxPHP\Shared\Counter(1);
} catch (\Throwable $e) {
    $err = $e;
}
$t->assertInstanceOf('constructing a Shared type throws Error', $err, \Error::class);
$t->assertSame(
    'the Error carries the documented message',
    $err?->getMessage(),
    'Class "OxPHP\\Shared\\Counter" not found'
);

// The documented exception to "the surface is gone", and the reason
// `interface_exists('OxPHP\Shared\Shareable')` is not a valid feature probe:
// the C extension registers this interface in MINIT, so it survives the
// switch with nothing left in the build that implements it.
$t->assertTrue(
    'the Shareable interface survives the switch',
    interface_exists('OxPHP\\Shared\\Shareable', false)
);

$t->done();
