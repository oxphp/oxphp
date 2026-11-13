<?php

declare(strict_types=1);

// Inner request for fibers/test_write_cancel_releases_frames, with
// ?in=stream-fiber for fibers/test_write_cancel_in_a_fiber_releases_frames, with
// ?in=stream-held-fiber for
// fibers/test_fatal_while_releasing_a_held_fiber_keeps_the_request_fiber, with
// ?in=stream-decorated for
// fibers/test_write_cancel_in_a_decorator_keeps_the_request_fiber_for_the_release,
// with ?in=stream-decorated-destructor for
// fibers/test_fatal_in_a_released_destructor_keeps_the_request_fiber_for_its_decorator,
// with ?in=stream-decorated-oom for
// fibers/test_oom_in_a_decorator_after_keeps_the_release,
// with ?in=stream-writing-destructor for
// fibers/test_write_cancel_release_runs_past_a_destructor_that_writes, with
// ?in=stream-fiber-starting-destructor for
// fibers/test_write_cancel_release_lets_a_destructor_start_a_fiber, and with
// ?in=stream-collects for
// fibers/test_collection_while_releasing_frames_keeps_the_worker.
//
// Parks while holding an object in a local variable. The test closes this
// request's connection during the park; when the request resumes, the first
// thing it does is write. A write belonging to a request whose client has gone
// used to end the request there; on a non-streaming worker-mode request it no
// longer does, and the script runs on to the end of its handler so that its own
// cleanup gets to run. Either way the write raises no error — nothing is
// reported, nothing is logged — which is what separates this from a
// cancellation delivered by interrupting the script.
//
// Everything the request was holding is the worker's to give back if the
// request does not give it back itself: the worker keeps serving, and what
// nobody releases stays allocated for the rest of its life.
//
// ?in=shutdown does the same from a shutdown function of a request that has
// already had a fatal. The engine runs shutdown functions under a guard of its
// own, so a write that ended the request there would end the shutdown function
// rather than the handler; it does not, and the function runs past it. A request
// that has had a fatal is filed differently by the worker, which must not change
// what it gives back.
//
// ?in=stream and ?in=stream-throw make the request a stream first. A stream is
// still ended at the write once its client has gone — its loop has no other
// bound — so these are the requests that take the path where the worker gives
// back what the abandoned frames were holding. stream-throw holds an object
// whose destructor throws: it runs during that give-back, and the throw must end
// nothing but itself.
//
// ?in=stream-fiber makes the same write from inside a Fiber the request starts
// after it resumes. The engine catches the ending inside that fiber and destroys
// the fiber's frames before it ends the request with it, so what the worker can
// give back is what the request's own frames below the fiber were holding — and
// the object is held there. Its destructor runs while the worker gives it back,
// and records the fiber it runs in: the request's, though the engine cleared its
// record of the current fiber when it ended the request from inside another.
//
// ?in=stream-held-fiber holds a Fiber suspended in a try whose finally fatals,
// and writes from the request itself. Destroying that fiber while the worker
// gives back what the frame held resumes it to run the finally, and the fatal
// there ends the give-back from inside that fiber, which clears the engine's
// record of the current fiber again. The request's shutdown function, after it,
// records the fiber it runs in.
//
// ?in=stream-decorated makes the ?in=stream-fiber write from inside a decorated
// function. The ending leaves that call open, and the worker closes it before it
// gives anything back, which calls the decorator's after(). after() records the
// fiber it runs in, then fatals inside a Fiber of its own — a write there would
// be dropped rather than end it, being the end of a request that is already
// over — and that ending clears the engine's record of the current fiber again
// on its way out of that fiber. The object the request held, given back after
// it, records the fiber its destructor runs in.
//
// ?in=stream-decorated-oom makes the ?in=stream write from inside a function
// whose decorator's after() runs out of memory. The worker closes that call
// first, as above, and the engine reports the error inside after(). The object
// the request held is given back after it; it holds a stream with a userland
// filter, and closing the stream calls the filter's onClose(), which records
// that it ran. Not a destructor: whether one may still run turns on whether the
// report got as far as flagging every object as destructed before its display,
// a write to the gone client, cut it short. A method the engine is asked to call
// does not depend on that flag.
//
// ?in=stream-decorated-destructor holds an object whose decorated destructor
// lets go of a Fiber suspended in a try whose finally fatals. The give-back runs
// that destructor, the fatal ends it from inside that fiber, and the worker
// closes the destructor's call after the give-back, which calls the same after():
// it records the fiber it runs in and ends itself with a write. The request's
// shutdown function, after it, records the fiber it runs in.
//
// ?in=stream-writing-destructor holds an object whose properties are two
// objects with destructors. The worker gives them back in order: the first
// writes to the same gone client and records that it returned from the write,
// the second records that it ran.
//
// ?in=stream-fiber-starting-destructor holds an object whose destructor starts
// a Fiber of its own, as a client built on fibers does when it closes. The
// fiber records that it ran, and the destructor that it got past the start.
//
// ?in=stream-collects leaves a cycle of objects with destructors for the
// collector right before the write, and holds an object whose destructor asks
// for a collection. The give-back runs that destructor, and the collection it
// asks for has a destructor of its own to call; what it saw goes to the file
// fibers/test_collection_while_releasing_frames_keeps_the_worker_probe reads.
//
// ?in=destructor holds an object with a destructor instead. The destructor now
// runs at the function's own return, as part of the request, because the
// request gets to reach that return. It sleeps there, which under the runtime
// hooks is a point a request can park at. It used to run inside the worker's
// cleanup instead: the ending marked no object destructed — a fatal does, but
// only once its message has been displayed, and not at all when that display is
// itself a write to a cancelled request — so the abandoned frame's locals were
// released by the frame walk afterwards, which ran it there.

require_once __DIR__ . '/write_cancel_probe.php';
require_once __DIR__ . '/bailout_leak_probe.php';
require_once __DIR__ . '/../engine_fatal.php';

if (!class_exists('OxphpWriteCancelDestructs', false)) {
    final class OxphpWriteCancelDestructs
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$stageAtDestruct = OxphpWriteCancelProbe::$stage;
            OxphpWriteCancelProbe::$stage = 'destructing';
            // Hooked. Long enough for the test's own poll to come round many
            // times, if this lets it.
            usleep(300_000);
            OxphpWriteCancelProbe::$stage = 'destructed';
        }
    }
}

if (!class_exists('OxphpWriteCancelThrows', false)) {
    final class OxphpWriteCancelThrows
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$stageAtDestruct = OxphpWriteCancelProbe::$stage;
            throw new \RuntimeException('thrown from a destructor during the give-back');
        }
    }
}

if (!class_exists('OxphpWriteCancelWritesThenRecords', false)) {
    final class OxphpWriteCancelWritesOnDestruct
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$stageAtDestruct = OxphpWriteCancelProbe::$stage;
            echo "nobody is left to read this destructor either\n";
            oxphp_stream_flush();
            OxphpWriteCancelProbe::$writerReturned = true;
        }
    }

    final class OxphpWriteCancelRecordsDestruct
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$nextDestructed = true;
        }
    }

    final class OxphpWriteCancelWritesThenRecords
    {
        // Released in declaration order when this object goes.
        private OxphpWriteCancelWritesOnDestruct $writes;
        private OxphpWriteCancelRecordsDestruct $records;

        public function __construct()
        {
            $this->writes = new OxphpWriteCancelWritesOnDestruct();
            $this->records = new OxphpWriteCancelRecordsDestruct();
        }
    }
}

if (!class_exists('OxphpWriteCancelStartsFiberOnDestruct', false)) {
    final class OxphpWriteCancelStartsFiberOnDestruct
    {
        public function __destruct()
        {
            OxphpWriteCancelProbe::$stageAtDestruct = OxphpWriteCancelProbe::$stage;
            (new \Fiber(static function (): void {
                OxphpWriteCancelProbe::$destructorFiberRan = true;
            }))->start();
            OxphpWriteCancelProbe::$stage = 'destructor-returned';
        }
    }
}

if (!class_exists('OxphpWriteCancelRecordsFiber', false)) {
    final class OxphpWriteCancelRecordsFiber
    {
        public function __destruct()
        {
            $current = \Fiber::getCurrent();
            OxphpWriteCancelProbe::$fiberAtDestruct = $current === null ? null : spl_object_id($current);
        }
    }
}

if (!class_exists('OxphpWriteCancelCycle', false)) {
    final class OxphpWriteCancelCycle
    {
        public ?self $peer = null;

        public static int $destructed = 0;

        public function __destruct()
        {
            self::$destructed++;
        }
    }
}

if (!class_exists('OxphpWriteCancelCollects', false)) {
    final class OxphpWriteCancelCollects
    {
        public function __destruct()
        {
            $before = gc_status();
            $collected = gc_collect_cycles();
            $after = gc_status();
            OxphpBailoutLeak::note('collect_in_release', 'gc', [
                'protected' => $before['protected'],
                'collected' => $collected,
                'running' => $after['running'],
                'cycle_destructed' => OxphpWriteCancelCycle::$destructed,
            ]);
        }
    }
}

if (!function_exists('oxphp_write_cancel_fiber_fataling_when_destroyed')) {
    function oxphp_write_cancel_fiber_fataling_when_destroyed(): \Fiber
    {
        $fiber = new \Fiber(static function (): void {
            try {
                \Fiber::suspend();
            } finally {
                engine_fatal('OxphpWriteCancelHeldFiberFinally');
            }
        });
        $fiber->start();

        return $fiber;
    }
}

if (!class_exists('OxphpWriteCancelDecorator', false)) {
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class OxphpWriteCancelDecorator implements \OxPHP\Decorator\AttributeInterface
    {
        public function before(\OxPHP\Decorator\Context $ctx): void
        {
        }

        public function after(\OxPHP\Decorator\Context $ctx): void
        {
            $current = \Fiber::getCurrent();
            OxphpWriteCancelProbe::$fiberAtAfter = $current === null ? null : spl_object_id($current);
            OxphpWriteCancelProbe::$afterStage = 'writing';
            (new \Fiber(static function (): void {
                echo "nobody is left to read this either\n";
                oxphp_stream_flush();
            }))->start();
            OxphpWriteCancelProbe::$afterStage = 'wrote';
        }
    }

    oxphp_register_decorator(OxphpWriteCancelDecorator::class);

    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class OxphpWriteCancelFatalDecorator implements \OxPHP\Decorator\AttributeInterface
    {
        public function before(\OxPHP\Decorator\Context $ctx): void
        {
        }

        public function after(\OxPHP\Decorator\Context $ctx): void
        {
            $current = \Fiber::getCurrent();
            OxphpWriteCancelProbe::$fiberAtAfter = $current === null ? null : spl_object_id($current);
            OxphpWriteCancelProbe::$afterStage = 'fataling';
            (new \Fiber(static function (): void {
                engine_fatal('OxphpWriteCancelDecoratorAfterFiberFatal');
            }))->start();
            OxphpWriteCancelProbe::$afterStage = 'returned';
        }
    }

    oxphp_register_decorator(OxphpWriteCancelFatalDecorator::class);
}

if (!class_exists('OxphpWriteCancelRecordsCloseFilter', false)) {
    class OxphpWriteCancelRecordsCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            OxphpWriteCancelProbe::$filterClosed = true;
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }

    final class OxphpWriteCancelHoldsFilteredStream
    {
        /** @var resource */
        private $stream;

        public function __construct()
        {
            $this->stream = fopen('php://memory', 'r+');
            // The chain takes a reference of its own, so discarding what this
            // returns leaves the filter alive until the stream is closed.
            stream_filter_append($this->stream, 'oxphp-write-cancel-records-close');
        }
    }
}

// The filter map is per thread and this worker keeps it for its whole life, so
// once a request on this worker has registered the name it stays registered.
if (!in_array('oxphp-write-cancel-records-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-write-cancel-records-close', OxphpWriteCancelRecordsCloseFilter::class);
}

if (!class_exists('OxphpWriteCancelOomDecorator', false)) {
    #[\Attribute(\Attribute::TARGET_FUNCTION | \Attribute::TARGET_METHOD)]
    final class OxphpWriteCancelOomDecorator implements \OxPHP\Decorator\AttributeInterface
    {
        public function before(\OxPHP\Decorator\Context $ctx): void
        {
        }

        public function after(\OxPHP\Decorator\Context $ctx): void
        {
            OxphpWriteCancelProbe::$afterStage = 'exhausting';
            $s = 'x';
            for (;;) {
                $s .= $s;
            }
        }
    }

    oxphp_register_decorator(OxphpWriteCancelOomDecorator::class);
}

if (!class_exists('OxphpWriteCancelReleasesFatalingFiber', false)) {
    final class OxphpWriteCancelReleasesFatalingFiber
    {
        private ?\Fiber $fiber;

        public function __construct()
        {
            $this->fiber = oxphp_write_cancel_fiber_fataling_when_destroyed();
        }

        #[OxphpWriteCancelDecorator]
        public function __destruct()
        {
            // The fiber's only reference: destroyed here, inside this call.
            $this->fiber = null;
            OxphpWriteCancelProbe::$stage = 'destructor-returned';
        }
    }
}

if (!function_exists('oxphp_write_after_client_left')) {
    function oxphp_write_after_client_left(string $holding = 'plain', bool $stream = false, bool $inFiber = false): void
    {
        // Held by this frame's variable and nothing else.
        $held = match ($holding) {
            'destructs' => new OxphpWriteCancelDestructs(),
            'throws' => new OxphpWriteCancelThrows(),
            'records-fiber' => new OxphpWriteCancelRecordsFiber(),
            'writes-then-records' => new OxphpWriteCancelWritesThenRecords(),
            'starts-fiber' => new OxphpWriteCancelStartsFiberOnDestruct(),
            'fiber-fataling-when-destroyed' => oxphp_write_cancel_fiber_fataling_when_destroyed(),
            'releases-fataling-fiber' => new OxphpWriteCancelReleasesFatalingFiber(),
            'collects' => new OxphpWriteCancelCollects(),
            'filtered-stream' => new OxphpWriteCancelHoldsFilteredStream(),
            default => new \ArrayObject([str_repeat('x', 1 << 16)]),
        };
        OxphpWriteCancelProbe::$weak = \WeakReference::create($held);

        OxphpWriteCancelProbe::$stage = 'parked';
        // Hooked: parks this request. The test takes the client away meanwhile.
        sleep(2);

        OxphpWriteCancelProbe::$stage = 'before-write';
        if ($holding === 'collects') {
            // After the park, so that no collection between requests on this
            // worker takes the cycle out of the buffer first.
            OxphpWriteCancelCycle::$destructed = 0;
            $a = new OxphpWriteCancelCycle();
            $b = new OxphpWriteCancelCycle();
            $a->peer = $b;
            $b->peer = $a;
            unset($a, $b);
        }
        $current = \Fiber::getCurrent();
        OxphpWriteCancelProbe::$requestFiber = $current === null ? null : spl_object_id($current);
        if ($inFiber) {
            (new \Fiber(static function (): void {
                echo "nobody is left to read this\n";
                oxphp_stream_flush();
            }))->start();
        } else {
            echo "nobody is left to read this\n";
            if ($stream) {
                oxphp_stream_flush();
            }
        }
        OxphpWriteCancelProbe::$stage = 'after-write';
    }
}

if (!function_exists('oxphp_write_after_client_left_decorated')) {
    #[OxphpWriteCancelFatalDecorator]
    function oxphp_write_after_client_left_decorated(): void
    {
        oxphp_write_after_client_left('records-fiber', true, true);
    }
}

if (!function_exists('oxphp_write_after_client_left_decorated_oom')) {
    #[OxphpWriteCancelOomDecorator]
    function oxphp_write_after_client_left_decorated_oom(): void
    {
        oxphp_write_after_client_left('filtered-stream', true);
    }
}

error_clear_last();

if (($_GET['in'] ?? '') === 'shutdown') {
    // The fatal below is not handed to an error handler. Cleared all the same,
    // because anything else raised on the way to it would be, and a handler an
    // earlier request on this worker installed may throw on it.
    set_error_handler(null);

    register_shutdown_function('oxphp_write_after_client_left');
    engine_fatal('OxphpFatalAheadOfTheShutdownFunction');
}

// Runs after the worker has dealt with the abandoned frames. Writes nothing:
// the client is gone and nobody would read it.
register_shutdown_function(static function (): void {
    $current = \Fiber::getCurrent();
    OxphpWriteCancelProbe::$report = [
        'stage' => OxphpWriteCancelProbe::$stage,
        'freed' => OxphpWriteCancelProbe::freed(),
        'last_error' => error_get_last()['message'] ?? null,
        'fiber' => $current === null ? null : spl_object_id($current),
    ];
});

$in = $_GET['in'] ?? '';
$stream = str_starts_with($in, 'stream');
if ($stream) {
    // The header alone is what makes the request a stream. Nothing is sent
    // before the park: a stream that has already sent its headers is no longer
    // watched for its client leaving until its next flush, which is a
    // different path out (see the test).
    header('Content-Type: text/event-stream');
}

if ($in === 'stream-decorated') {
    oxphp_write_after_client_left_decorated();
} elseif ($in === 'stream-decorated-oom') {
    oxphp_write_after_client_left_decorated_oom();
} else {
    oxphp_write_after_client_left(
        match ($in) {
            'destructor' => 'destructs',
            'stream-throw' => 'throws',
            'stream-fiber' => 'records-fiber',
            'stream-held-fiber' => 'fiber-fataling-when-destroyed',
            'stream-decorated-destructor' => 'releases-fataling-fiber',
            'stream-writing-destructor' => 'writes-then-records',
            'stream-fiber-starting-destructor' => 'starts-fiber',
            'stream-collects' => 'collects',
            default => 'plain',
        },
        $stream,
        $in === 'stream-fiber'
    );
}
