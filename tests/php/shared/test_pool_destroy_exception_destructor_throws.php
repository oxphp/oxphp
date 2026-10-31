<?php
/**
 * An exception $destroy throws, dropped with no other exception in flight.
 *
 * Dropping it can run a destructor — its own, or that of something it holds,
 * a second pool included — and that is user code. A destructor that throws
 * must throw at the statement that let the pool go, as one run there by PHP
 * itself would, and with nothing thrown the code must go on past that
 * statement. Dropped while the frame is still marked as handling it, the
 * exception sends the frame on from an instruction of the destructor's
 * function, and the process crashes or the exception misses its catch.
 *
 * Closures and anonymous classes rather than named declarations: in worker
 * mode this file is included once per request.
 */

header('Content-Type: text/plain');

use OxPHP\Shared\Pool;

// A pool with $idle idle slots, so that letting go of it calls $destroy that
// many times.
$poolWithIdleSlots = static function (callable $destroy, int $idle = 1): Pool {
    $pool = new Pool(
        factory: static fn (): object => new stdClass(),
        destroy: $destroy,
        maxSize: $idle,
    );
    $handles = [];
    for ($i = 0; $i < $idle; $i++) {
        $handles[] = $pool->acquire();
    }
    foreach ($handles as $handle) {
        $handle->release();
    }
    return $pool;
};

// A $destroy that throws an exception whose destructor throws in turn.
$destroyed = 0;
$throwsThrowingException = static function (object $r) use (&$destroyed): void {
    $destroyed++;
    throw new class ('from destroy') extends LogicException {
        public function __destruct()
        {
            throw new DomainException('from destructor');
        }
    };
};

$describe = static fn (?Throwable $e): string =>
    $e === null ? 'nothing' : get_class($e) . ': ' . $e->getMessage();

// ── Scenario 1: $destroy throws an exception with no destructor ──
// It is dropped: nothing reaches the code that let the pool go, which goes on
// with the next statement, once.
$destroyed = 0;
$pool = $poolWithIdleSlots(static function (object $r) use (&$destroyed): void {
    $destroyed++;
    throw new LogicException('from destroy');
});
$caught = null;
$ranOn = 0;
try {
    unset($pool);
    $ranOn++;
} catch (Throwable $e) {
    $caught = $e;
}
if ($caught !== null) {
    echo 'FAIL: scenario 1 caught ', $describe($caught), "\n";
    exit;
}
if ($ranOn !== 1) {
    echo "FAIL: scenario 1 ran the statement after unset() {$ranOn} times, expected 1\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 1 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}

// ── Scenario 2: its destructor throws, the pool is let go of by unset() ──
// The destructor's exception is thrown at the unset(): the statement after it
// does not run and the catch around it gets the exception, with nothing chained
// to it — nothing else was in flight.
$destroyed = 0;
$pool = $poolWithIdleSlots($throwsThrowingException);
$caught = null;
$ranOn = false;
try {
    unset($pool);
    $ranOn = true;
} catch (Throwable $e) {
    $caught = $e;
}
if ($ranOn) {
    echo "FAIL: scenario 2 went on past unset()\n";
    exit;
}
if (!$caught instanceof DomainException || $caught->getMessage() !== 'from destructor') {
    echo 'FAIL: scenario 2 caught ', $describe($caught), "\n";
    exit;
}
if ($caught->getPrevious() !== null) {
    echo 'FAIL: scenario 2 exception has a previous ', $describe($caught->getPrevious()), "\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 2 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}

// ── Scenario 3: the same, the pool a local of a function that returns ──
// PHP frees the local with the caller's frame already current, so the
// exception is thrown at the call.
$destroyed = 0;
$holdAndReturn = static function () use ($poolWithIdleSlots, $throwsThrowingException): void {
    $pool = $poolWithIdleSlots($throwsThrowingException);
};
$caught = null;
$ranOn = false;
try {
    $holdAndReturn();
    $ranOn = true;
} catch (Throwable $e) {
    $caught = $e;
}
if ($ranOn) {
    echo "FAIL: scenario 3 went on past the call\n";
    exit;
}
if (!$caught instanceof DomainException || $caught->getMessage() !== 'from destructor') {
    echo 'FAIL: scenario 3 caught ', $describe($caught), "\n";
    exit;
}
if ($caught->getPrevious() !== null) {
    echo 'FAIL: scenario 3 exception has a previous ', $describe($caught->getPrevious()), "\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 3 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}

// ── Scenario 4: two idle slots let go of by one unset() ──
// The second slot's $destroy runs with the first destructor's exception in
// flight, so the two come out chained, the last one first — as from objects
// whose destructors throw one after another.
$destroyed = 0;
$pool = $poolWithIdleSlots($throwsThrowingException, 2);
$caught = null;
$ranOn = false;
try {
    unset($pool);
    $ranOn = true;
} catch (Throwable $e) {
    $caught = $e;
}
if ($ranOn) {
    echo "FAIL: scenario 4 went on past unset()\n";
    exit;
}
if ($destroyed !== 2) {
    echo "FAIL: scenario 4 ran \$destroy {$destroyed} times, expected 2\n";
    exit;
}
$chain = [];
for ($link = $caught; $link !== null; $link = $link->getPrevious()) {
    $chain[] = $describe($link);
}
if ($chain !== array_fill(0, 2, 'DomainException: from destructor')) {
    echo 'FAIL: scenario 4 caught ', $chain === [] ? 'nothing' : implode(' <- ', $chain), "\n";
    exit;
}

// ── Scenario 5: the exception holds the last reference to another pool ──
// Dropping it lets go of that pool, whose $destroy then runs, throws an
// exception with no destructor and has it dropped in turn. Nothing reaches the
// code that let the first pool go, which goes on with the next statement, once.
$outerDestroyed = 0;
$innerDestroyed = 0;
$pool = $poolWithIdleSlots(static function (object $r) use ($poolWithIdleSlots, &$outerDestroyed, &$innerDestroyed): void {
    $outerDestroyed++;
    $inner = $poolWithIdleSlots(static function (object $r) use (&$innerDestroyed): void {
        $innerDestroyed++;
        throw new LogicException('from the inner destroy');
    });
    throw new class ($inner) extends LogicException {
        public function __construct(public Pool $pool)
        {
            parent::__construct('from the outer destroy');
        }
    };
});
$caught = null;
$ranOn = 0;
try {
    unset($pool);
    $ranOn++;
} catch (Throwable $e) {
    $caught = $e;
}
if ($caught !== null) {
    echo 'FAIL: scenario 5 caught ', $describe($caught), "\n";
    exit;
}
if ($ranOn !== 1) {
    echo "FAIL: scenario 5 ran the statement after unset() {$ranOn} times, expected 1\n";
    exit;
}
if ($outerDestroyed !== 1 || $innerDestroyed !== 1) {
    echo "FAIL: scenario 5 ran the outer \$destroy {$outerDestroyed} times and the inner {$innerDestroyed}, expected 1 and 1\n";
    exit;
}

// ── Scenario 6: the slot is let go of by evict() ──
// The frame current then is evict()'s own, a built-in function's; the
// exception leaves evict() into the catch around it.
$destroyed = 0;
$pool = $poolWithIdleSlots($throwsThrowingException);
$caught = null;
$ranOn = false;
try {
    $pool->evict();
    $ranOn = true;
} catch (Throwable $e) {
    $caught = $e;
}
if ($ranOn) {
    echo "FAIL: scenario 6 went on past evict()\n";
    exit;
}
if (!$caught instanceof DomainException || $caught->getMessage() !== 'from destructor') {
    echo 'FAIL: scenario 6 caught ', $describe($caught), "\n";
    exit;
}
if ($caught->getPrevious() !== null) {
    echo 'FAIL: scenario 6 exception has a previous ', $describe($caught->getPrevious()), "\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 6 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}
unset($pool);

echo "OK\n";
