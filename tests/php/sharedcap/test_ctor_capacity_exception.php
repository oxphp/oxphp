<?php
declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Atomic;
use OxPHP\Shared\CapacityException;
use OxPHP\Shared\Channel;
use OxPHP\Shared\Counter;
use OxPHP\Shared\Flag;
use OxPHP\Shared\Map;
use OxPHP\Shared\Mutex;
use OxPHP\Shared\Once;
use OxPHP\Shared\Pool;

// SHARED_MAX_ENTRIES=16: once the registry holds that many live objects, every
// constructor must refuse with CapacityException — the class the caps are
// documented to throw — and not with its SharedException parent, which a
// `catch (CapacityException)` does not see.
$t = new TestCase('ctor_capacity_exception', 'sharedcap');

// Fill the registry. The array keeps every object alive until the end of the
// request, so the cap stays reached for the constructors below.
$held = [];
$fillError = null;
for ($i = 0; $i < 1000; $i++) {
    try {
        $held[] = new Map();
    } catch (\Throwable $e) {
        $fillError = $e;
        break;
    }
}

// Premise: the loop stopped on the global cap, not on some other failure and
// not by running out of iterations. Without it the checks below would pass or
// fail for a reason that has nothing to do with the cap.
$t->assertGreaterThan('objects created before the cap', count($held), 0);
$t->assertSame('fill loop stopped on CapacityException', $fillError === null ? null : get_class($fillError), CapacityException::class);
$t->assertContains('fill loop refusal is the entries cap', $fillError?->getMessage() ?? '', 'capacity exceeded');

$constructors = [
    'Counter' => fn () => new Counter(),
    'Atomic'  => fn () => new Atomic(),
    'Flag'    => fn () => new Flag(),
    'Once'    => fn () => new Once(),
    'Mutex'   => fn () => new Mutex(0),
    'Channel' => fn () => new Channel(1),
    'Map'     => fn () => new Map(),
    'Pool'    => fn () => new Pool(fn () => 1),
];
foreach ($constructors as $type => $make) {
    $caught = null;
    try {
        $held[] = $make();
    } catch (\Throwable $e) {
        $caught = $e;
    }
    // Exact class, not instanceof: CapacityException extends SharedException,
    // so an instanceof check on the parent passes on exactly the bug.
    $t->assertSame("$type refused with CapacityException", $caught === null ? null : get_class($caught), CapacityException::class);
    $t->assertContains("$type refusal is the entries cap", $caught?->getMessage() ?? '', 'capacity exceeded');
}

$t->done();
