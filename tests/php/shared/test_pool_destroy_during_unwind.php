<?php
/**
 * $destroy of a pool let go of while an exception unwinds the frame holding it.
 *
 * The pool's last reference is a local of a function that throws, so the pool
 * goes away — and hands its idle slot to $destroy — while PHP unwinds that
 * frame with the exception in flight. $destroy must run, as a destructor does
 * there, and the exception must reach the caller's catch as it was thrown. A
 * build that cleared the in-flight exception as if $destroy had thrown it
 * crashed here, or resumed the caller in the middle of the thrower's code.
 *
 * Closures and an anonymous class rather than named declarations: in worker
 * mode this file is included once per request.
 */

header('Content-Type: text/plain');

use OxPHP\Shared\Pool;

// A pool with one idle slot, so that letting go of it calls $destroy once.
$poolWithIdleSlot = static function (callable $destroy): Pool {
    $pool = new Pool(
        factory: static fn (): object => new stdClass(),
        destroy: $destroy,
        maxSize: 1,
    );
    $pool->acquire()->release();
    return $pool;
};

// ── Scenario 1: the pool is a local of the function that throws ──
// The exception caught must be the one thrown — same class, message and line,
// nothing chained to it — and $destroy must have run once.
$destroyed = 0;
$throwLine = 0;
$holdAndThrow = static function () use ($poolWithIdleSlot, &$destroyed, &$throwLine): void {
    $pool = $poolWithIdleSlot(static function (object $r) use (&$destroyed): void {
        $destroyed++;
    });
    $throwLine = __LINE__; throw new RuntimeException('mine');
};
$caught = null;
try {
    $holdAndThrow();
} catch (Throwable $e) {
    $caught = $e;
}
if (!$caught instanceof RuntimeException || $caught->getMessage() !== 'mine') {
    echo 'FAIL: scenario 1 caught ', $caught === null ? 'nothing' : get_class($caught) . ': ' . $caught->getMessage(), "\n";
    exit;
}
if ($caught->getLine() !== $throwLine) {
    echo "FAIL: scenario 1 exception reports line {$caught->getLine()}, thrown at {$throwLine}\n";
    exit;
}
if ($caught->getPrevious() !== null) {
    echo 'FAIL: scenario 1 exception has a previous ', get_class($caught->getPrevious()), "\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 1 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}

// ── Scenario 2: what $destroy sees of the frame being unwound ──
// A backtrace taken inside it shows the caller at the call it is unwinding
// from, as one taken in a destructor does there — even after $destroy has
// thrown and caught an exception of its own.
$traceLine = null;
$holdAndThrow = static function () use ($poolWithIdleSlot, &$traceLine): void {
    $pool = $poolWithIdleSlot(static function (object $r) use (&$traceLine): void {
        try {
            throw new LogicException('handled inside $destroy');
        } catch (LogicException) {
        }
        $traceLine = (new Exception())->getTrace()[0]['line'] ?? null;
    });
    throw new RuntimeException('mine');
};
$callLine = 0;
try {
    $callLine = __LINE__; $holdAndThrow();
} catch (Throwable $e) {
}
if ($traceLine !== $callLine) {
    echo 'FAIL: scenario 2 backtrace in $destroy shows the caller at line ', var_export($traceLine, true), ", the call is at {$callLine}\n";
    exit;
}

// ── Scenario 3: $destroy throws while the exception is in flight ──
// Its own exception is dropped, as anywhere else it runs; the one in flight
// still reaches the catch, with nothing chained to it.
$destroyReached = false;
$holdAndThrow = static function () use ($poolWithIdleSlot, &$destroyReached): void {
    $pool = $poolWithIdleSlot(static function (object $r) use (&$destroyReached): void {
        $destroyReached = true;
        throw new LogicException('from destroy');
    });
    throw new RuntimeException('mine');
};
$caught = null;
try {
    $holdAndThrow();
} catch (Throwable $e) {
    $caught = $e;
}
if (!$caught instanceof RuntimeException || $caught->getMessage() !== 'mine') {
    echo 'FAIL: scenario 3 caught ', $caught === null ? 'nothing' : get_class($caught) . ': ' . $caught->getMessage(), "\n";
    exit;
}
if ($caught->getPrevious() !== null) {
    echo 'FAIL: scenario 3 exception has a previous ', get_class($caught->getPrevious()), "\n";
    exit;
}
if (!$destroyReached) {
    echo "FAIL: scenario 3 did not run \$destroy\n";
    exit;
}

// ── Scenario 4: the pool is a temporary of the throwing frame ──
// The array under construction holds it when the throw expression runs, so the
// engine frees it while that frame itself handles the exception.
$destroyed = 0;
$throwFromTemporary = static function () use ($poolWithIdleSlot, &$destroyed): array {
    return [
        $poolWithIdleSlot(static function (object $r) use (&$destroyed): void {
            $destroyed++;
        }),
        throw new RuntimeException('temporary'),
    ];
};
$caught = null;
try {
    $throwFromTemporary();
} catch (Throwable $e) {
    $caught = $e;
}
if (!$caught instanceof RuntimeException || $caught->getMessage() !== 'temporary') {
    echo 'FAIL: scenario 4 caught ', $caught === null ? 'nothing' : get_class($caught) . ': ' . $caught->getMessage(), "\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 4 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}

// ── Scenario 5: dropping $destroy's exception runs a destructor that throws ──
// That exception is thrown while the one in flight is set aside, so it takes
// its place with the original chained as previous — what PHP does with an
// exception a destructor throws while another unwinds the stack.
$holdAndThrow = static function () use ($poolWithIdleSlot): void {
    $pool = $poolWithIdleSlot(static function (object $r): void {
        throw new class ('from destroy') extends LogicException {
            public function __destruct()
            {
                throw new DomainException('from destructor');
            }
        };
    });
    throw new RuntimeException('mine');
};
$caught = null;
try {
    $holdAndThrow();
} catch (Throwable $e) {
    $caught = $e;
}
if (!$caught instanceof DomainException || $caught->getMessage() !== 'from destructor') {
    echo 'FAIL: scenario 5 caught ', $caught === null ? 'nothing' : get_class($caught) . ': ' . $caught->getMessage(), "\n";
    exit;
}
$previous = $caught->getPrevious();
if (!$previous instanceof RuntimeException || $previous->getMessage() !== 'mine') {
    echo 'FAIL: scenario 5 previous is ', $previous === null ? 'nothing' : get_class($previous) . ': ' . $previous->getMessage(), "\n";
    exit;
}

// ── Scenario 6: the frame is an internal function's ──
// array_map() keeps what the callback returned in the array it is building and
// destroys that array in its own frame when a later call throws, so the pool
// goes away there with the exception in flight. The exception must still reach
// the catch, and nothing after the array_map() call may run.
$destroyed = 0;
$caught = null;
$ranOn = false;
try {
    array_map(static function (int $i) use ($poolWithIdleSlot, &$destroyed): Pool {
        if ($i === 2) {
            throw new RuntimeException('from the callback');
        }
        return $poolWithIdleSlot(static function (object $r) use (&$destroyed): void {
            $destroyed++;
        });
    }, [1, 2]);
    $ranOn = true;
} catch (Throwable $e) {
    $caught = $e;
}
if ($ranOn) {
    echo "FAIL: scenario 6 went on past array_map()\n";
    exit;
}
if (!$caught instanceof RuntimeException || $caught->getMessage() !== 'from the callback') {
    echo 'FAIL: scenario 6 caught ', $caught === null ? 'nothing' : get_class($caught) . ': ' . $caught->getMessage(), "\n";
    exit;
}
if ($caught->getPrevious() !== null) {
    echo 'FAIL: scenario 6 exception has a previous ', get_class($caught->getPrevious()), "\n";
    exit;
}
if ($destroyed !== 1) {
    echo "FAIL: scenario 6 ran \$destroy {$destroyed} times, expected 1\n";
    exit;
}

// ── Scenario 7: the exception is one the pool's own teardown left ──
// The pooled object's destructor throws as its slot is freed, so evict() calls
// the next slot's $destroy with that exception in flight. Every slot must
// still get $destroy, and the exceptions come out of evict() chained, the last
// one first — as from objects whose destructors throw one after another.
$destroyed = 0;
$pool = new Pool(
    factory: static fn (): object => new class () {
        public function __destruct()
        {
            throw new RuntimeException('from a pooled object');
        }
    },
    destroy: static function (object $r) use (&$destroyed): void {
        $destroyed++;
    },
    maxSize: 2,
);
$first = $pool->acquire();
$second = $pool->acquire();
$first->release();
$second->release();
unset($first, $second);
$caught = null;
try {
    $pool->evict();
} catch (Throwable $e) {
    $caught = $e;
}
if ($destroyed !== 2) {
    echo "FAIL: scenario 7 ran \$destroy {$destroyed} times, expected 2\n";
    exit;
}
$chain = [];
for ($link = $caught; $link !== null; $link = $link->getPrevious()) {
    $chain[] = get_class($link) . ': ' . $link->getMessage();
}
if ($chain !== array_fill(0, 2, 'RuntimeException: from a pooled object')) {
    echo 'FAIL: scenario 7 evict() threw ', $chain === [] ? 'nothing' : implode(' <- ', $chain), "\n";
    exit;
}
unset($pool);

// ── Scenario 8 (last: it ends the script): exit() in the function ──
// exit() unwinds the frame the same way. $destroy prints the OK, so the test
// passes only if it ran; a build that swallowed exit() goes on to the FAIL.
$holdAndExit = static function () use ($poolWithIdleSlot): void {
    $pool = $poolWithIdleSlot(static function (object $r): void {
        echo "OK\n";
    });
    exit;
};
$holdAndExit();
echo "FAIL: exit() in a function holding a pool did not end the script\n";
