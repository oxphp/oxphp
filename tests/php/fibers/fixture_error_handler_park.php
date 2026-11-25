<?php

declare(strict_types=1);

// Inner self-request for the fibers/test_error_handler_*parked* tests.
//
// Installs an error handler that throws an ErrorException of its own for every
// error but the ones below, raises the one it parks on, and parks — each park
// published under its own tag for the test to read at the moment it measures.
//
//   ?mode=phases  raises a few errors the handler hears and returns from, then
//                 parks three times: inside the handler; inside it again, past a
//                 set_error_handler(null) of its own; and once the handler has
//                 returned, with no handler installed at the depth it ran at, in
//                 a stream wrapper's open whose frame takes the place the
//                 handler's had. Leaves the worker's handlers as it found them.
//   ?mode=callable has a handler that is not a Closure heard once and return,
//                 then parks as the last park of `phases` does, in a callback
//                 that C calls with one argument rather than four. With
//                 &via=open, in a stream wrapper's open instead, which C calls
//                 with the four a handler gets.
//   ?mode=forms   parks once, inside a handler that is not a Closure, of the
//                 callable form &form= names: a method named in another case, a
//                 static method as a "Class::method" string, a function named
//                 with a leading backslash, an invokable object, or a trait's
//                 method under an alias.
//   ?mode=drop    has a handler heard once, then drops it: the handler is the
//                 last thing holding an object that marks itself released as it
//                 goes. No park.
//   ?mode=single  parks once, inside the handler, and leaves the worker with
//                 whatever handler stands once the handler has returned.
//   ?mode=next    as `drop`, with an object that throws as it goes while armed,
//                 then raises another error, armed, under a handler of its own,
//                 and reports whether the object was still held at that error,
//                 what the error threw and what the handler heard. No park.
//   ?mode=pages   raises its error again and again, from a frame four arguments
//                 wider each time, so the handler's frame steps across a VM stack
//                 page four slots at a time and past its end on some steps, where
//                 PHP starts it on a new page. Parks inside the handler each time
//                 until the test has had its turn.
//   ?mode=late    leaves an argument in the backtrace of an E_USER_ERROR its
//                 handler takes over. On PHP 8.5 and later that backtrace holds
//                 it until the end of the request lets go of it, and its
//                 destructor raises an error the handler hears there, once the
//                 request's own code is over, then drops the handler: the last
//                 thing holding an object that marks itself released as it goes.
//                 With no such backtrace the destructor runs as the call returns.
//                 No park.
//   ?mode=saturate has nine handler objects, not Closures, each set, heard once
//                 from the same statement and removed — one more call than the
//                 server keeps a record of while the copies of removed handlers
//                 are kept. The ninth parks inside its call, whose frame lands
//                 where the eighth's did.
//   ?mode=via-call has a handler that, inside its call, puts back the one under
//                 it and installs one PHP reaches through __call(), then raises
//                 an error that one hears and parks in. Reports the handler that
//                 stands once both calls have returned, then puts the worker's
//                 back.
//   ?mode=inner-returned has a handler that, inside its call, installs one of its
//                 own, raises an error in its own frame that one hears, takes it
//                 off again and parks in a plain call. Leaves the worker's
//                 handlers as it found them.

require_once __DIR__ . '/error_mode_probe.php';

set_exception_handler(null);
header('Content-Type: text/plain');

if (!class_exists('OxphpErrorHandlerParkWrapper', false)) {
    final class OxphpErrorHandlerParkWrapper
    {
        /** @var resource|null Set by PHP on every wrapper instance. */
        public $context;

        public static ?\Closure $park = null;

        // Called from C with four arguments, as the error handler is.
        public function stream_open(string $path, string $mode, int $options, ?string &$openedPath): bool
        {
            (self::$park)(substr($path, strlen('oxehslot://')));

            return true;
        }

        public function stream_close(): void
        {
        }
    }
}
if (!in_array('oxehslot', stream_get_wrappers(), true)) {
    stream_wrapper_register('oxehslot', OxphpErrorHandlerParkWrapper::class);
}

if (!class_exists('OxphpErrorHandlerLateCaller', false)) {
    final class OxphpErrorHandlerLateCaller
    {
        public function __construct(private string $run)
        {
        }

        // Raises an error the handler hears, then drops the handler.
        public function __destruct()
        {
            trigger_error("late-{$this->run}", E_USER_NOTICE);
            restore_error_handler();
        }
    }
}

if (!class_exists('OxphpErrorHandlerCollector', false)) {
    final class OxphpErrorHandlerCollector
    {
        public function __construct(public readonly int $id, private string $run, private ?\Closure $park)
        {
        }

        public function __invoke(int $severity, string $message, string $file = '', int $line = 0): bool
        {
            if ($this->park !== null && $message === "collect-{$this->run}") {
                ($this->park)('saturated');
            }

            return true;
        }
    }

    final class OxphpErrorHandlerViaCall
    {
        public function __construct(private string $run, private \Closure $park)
        {
        }

        public function __call(string $name, array $args): bool
        {
            if (($args[1] ?? '') === "inner-{$this->run}") {
                ($this->park)('via-call');
            }

            return true;
        }
    }
}

if (!class_exists('OxphpErrorHandlerForms', false)) {
    trait OxphpErrorHandlerFormsTrait
    {
        public function heard(int $severity, string $message): bool
        {
            return OxphpErrorHandlerForms::onError($severity, $message);
        }
    }

    final class OxphpErrorHandlerForms
    {
        use OxphpErrorHandlerFormsTrait {
            heard as aliased;
        }

        public static string $form = '';
        public static string $run = '';
        public static ?\Closure $park = null;

        public static function onError(int $severity, string $message): bool
        {
            if ($message !== 'park-' . self::$run) {
                throw new \ErrorException('form-handler:' . self::$form . ':' . $message);
            }
            (self::$park)('form');

            return true;
        }

        public function handle(int $severity, string $message): bool
        {
            return self::onError($severity, $message);
        }

        public function __invoke(int $severity, string $message): bool
        {
            return self::onError($severity, $message);
        }
    }

    function oxphp_error_handler_forms_handler(int $severity, string $message): bool
    {
        return OxphpErrorHandlerForms::onError($severity, $message);
    }
}

$run = (string) ($_GET['run'] ?? '?');
$phases = ($_GET['mode'] ?? '') === 'phases';
$reentered = 0;

$park = static function (string $what) use ($run): void {
    OxphpErrorModeProbe::$inWindow["$what-$run"] = true;
    // Hooked: parks this request.
    usleep(400_000);
    OxphpErrorModeProbe::$inWindow["$what-$run"] = false;
};

$handler = static function (int $severity, string $message, string $file = '', int $line = 0) use ($run, $phases, $park, &$reentered): bool {
    if ($message === "park-$run") {
        $park('in-handler');
        if ($phases) {
            // The engine runs a handler with none installed, so an error the
            // handler itself raises is not handed back to it. Not in `single`,
            // where the test sets a handler of its own meanwhile: one raised
            // here goes to that.
            trigger_error("inside-$run", E_USER_NOTICE);
            set_error_handler(null);
            $park('nulled');
            restore_error_handler();
        }

        return true;
    }
    if ($message === "inside-$run" || $message === "warm-$run") {
        $reentered += (int) ($message === "inside-$run");

        return true;
    }

    throw new \ErrorException("parked-handler:$message", 0, $severity, $file, $line);
};
if (($_GET['mode'] ?? '') === 'callable') {
    $handlerObject = new class ($run) {
        public int $heard = 0;

        public function __construct(private string $run)
        {
        }

        public function handle(int $severity, string $message, string $file = '', int $line = 0): bool
        {
            if ($message !== "heard-{$this->run}") {
                throw new \ErrorException("callable-handler:$message", 0, $severity, $file, $line);
            }
            $this->heard++;

            return true;
        }
    };
    set_error_handler(static fn (): bool => false);
    set_error_handler([$handlerObject, 'handle']);
    trigger_error("heard-$run", E_USER_NOTICE);
    restore_error_handler();
    set_error_handler(null);
    // array_map() and fopen() take as many arguments as trigger_error() above,
    // so the frame of the callback they run lands where the handler's did,
    // entered from C over a frame where the error was raised.
    if (($_GET['via'] ?? '') === 'open') {
        OxphpErrorHandlerParkWrapper::$park = $park;
        $stream = fopen('oxehslot://callback', 'r');
        fclose($stream);
    } else {
        array_map($park, ['callback']);
    }
    restore_error_handler();
    restore_error_handler();
    echo 'HEARD:', $handlerObject->heard, "\n";
    echo "DONE\n";

    return;
}

if (($_GET['mode'] ?? '') === 'forms') {
    $handlerObject = new OxphpErrorHandlerForms();
    OxphpErrorHandlerForms::$form = (string) ($_GET['form'] ?? '');
    OxphpErrorHandlerForms::$run = $run;
    OxphpErrorHandlerForms::$park = $park;
    set_error_handler(match (OxphpErrorHandlerForms::$form) {
        'method' => [$handlerObject, 'HANDLE'],
        'static' => 'OxphpErrorHandlerForms::onError',
        'function' => '\oxphp_error_handler_forms_handler',
        'invoke' => $handlerObject,
        'alias' => [$handlerObject, 'aliased'],
    });
    trigger_error("park-$run", E_USER_WARNING);
    restore_error_handler();
    echo "DONE\n";

    return;
}

if (($_GET['mode'] ?? '') === 'pages') {
    $parks = 0;
    set_error_handler(static function (int $severity, string $message, string $file = '', int $line = 0) use ($run, &$parks): bool {
        if ($message !== "page-$run") {
            throw new \ErrorException("parked-handler:$message", 0, $severity, $file, $line);
        }
        $parks++;
        OxphpErrorModeProbe::$inWindow["page-$run"] = true;
        for ($i = 0; $i < 2_000 && !(OxphpErrorModeProbe::$inWindow["probed-$run"] ?? false); $i++) {
            usleep(1_000);
        }
        OxphpErrorModeProbe::$inWindow["probed-$run"] = false;
        OxphpErrorModeProbe::$inWindow["page-$run"] = false;

        return true;
    });
    $raise = static function () use ($run): void {
        trigger_error("page-$run", E_USER_NOTICE);
    };
    // A page is 1024 slots inside a fiber; the handler's frame is wider than a step.
    for ($width = 0; $width <= 1100; $width += 4) {
        $raise(...array_fill(0, $width, 0));
    }
    restore_error_handler();
    OxphpErrorModeProbe::$inWindow["pages-done-$run"] = true;
    echo 'PARKS:', $parks, "\n";
    echo "DONE\n";

    return;
}

if (($_GET['mode'] ?? '') === 'next') {
    $marker = new OxphpErrorModeReleaseMarker("released-$run", "armed-$run");
    set_error_handler(static function () use ($marker): bool {
        return true;
    });
    unset($marker);
    trigger_error("dropped-$run", E_USER_NOTICE);
    restore_error_handler();
    $held = !(OxphpErrorModeProbe::$inWindow["released-$run"] ?? false);

    $heard = [];
    set_error_handler(static function (int $severity, string $message) use (&$heard): bool {
        $heard[] = $message;

        return true;
    });
    OxphpErrorModeProbe::$inWindow["armed-$run"] = true;
    try {
        trigger_error("next-$run", E_USER_NOTICE);
        $thrown = 'none';
    } catch (\Throwable $e) {
        $thrown = get_class($e) . ':' . $e->getMessage();
    }
    OxphpErrorModeProbe::$inWindow["armed-$run"] = false;
    restore_error_handler();
    echo 'HELD:', $held ? 'yes' : 'no', "\n";
    echo 'THROWN:', $thrown, "\n";
    echo 'HEARD:', implode(',', $heard), "\n";

    return;
}

if (($_GET['mode'] ?? '') === 'late') {
    $marker = new OxphpErrorModeReleaseMarker("released-$run");
    set_error_handler(static function (int $severity, string $message) use ($marker, $run): bool {
        if ($message === "late-$run") {
            OxphpErrorModeProbe::$inWindow["late-after-return-$run"] = OxphpErrorModeProbe::$inWindow["returned-$run"] ?? false;
        }

        return true;
    });
    unset($marker);
    // The handler takes over the deprecation E_USER_ERROR raises as well. No
    // error after this one: the next would let go of its backtrace.
    (static function (OxphpErrorHandlerLateCaller $late) use ($run): void {
        trigger_error("fatal-$run", E_USER_ERROR);
    })(new OxphpErrorHandlerLateCaller($run));
    OxphpErrorModeProbe::$inWindow["returned-$run"] = true;
    echo "ARMED\n";

    return;
}

if (($_GET['mode'] ?? '') === 'saturate') {
    for ($i = 1; $i <= 9; $i++) {
        set_error_handler(new OxphpErrorHandlerCollector($i, $run, $i === 9 ? $park : null));
        trigger_error("collect-$run", E_USER_NOTICE);
        restore_error_handler();
    }
    echo "DONE\n";

    return;
}

if (($_GET['mode'] ?? '') === 'via-call') {
    $viaCall = new OxphpErrorHandlerViaCall($run, $park);
    set_error_handler(static function (int $severity, string $message) use ($run, $viaCall): bool {
        if ($message === "outer-$run") {
            restore_error_handler();
            set_error_handler([$viaCall, 'viaCall']);
            trigger_error("inner-$run", E_USER_NOTICE);
        }

        return true;
    });
    trigger_error("outer-$run", E_USER_NOTICE);
    $after = set_error_handler(null);
    restore_error_handler();
    echo 'HANDLER-AFTER:', is_array($after) && $after[0] === $viaCall ? 'via-call' : get_debug_type($after), "\n";
    restore_error_handler();
    echo "DONE\n";

    return;
}

if (($_GET['mode'] ?? '') === 'inner-returned') {
    set_error_handler(static function (int $severity, string $message) use ($run, $park): bool {
        if ($message === "outer-$run") {
            set_error_handler(static fn (): bool => true);
            // Raised by the engine in this frame, where the inner handler's call is entered from.
            $unset = $notSet;
            restore_error_handler();
            $park('inner-returned');
        }

        return true;
    });
    trigger_error("outer-$run", E_USER_NOTICE);
    restore_error_handler();
    echo "DONE\n";

    return;
}

if (($_GET['mode'] ?? '') === 'drop') {
    $marker = new OxphpErrorModeReleaseMarker("released-$run");
    set_error_handler(static function () use ($marker): bool {
        return true;
    });
    unset($marker);
    trigger_error("heard-$run", E_USER_NOTICE);
    restore_error_handler();
    echo "DROPPED\n";

    return;
}

if ($phases) {
    // Under the handler, one that is never called. The last park below runs at
    // the mask this one was set with, which is what a handler lent there would
    // be heard at.
    set_error_handler(static fn (): bool => false);
}
set_error_handler($handler);

if ($phases) {
    // More calls than the server keeps a record of at once, each returned from
    // before the next, so the one that parks has room only once they are let go.
    // Made from a frame of their own, so that none of them stands where the call
    // that parks will.
    $warm = static function () use ($run): void {
        trigger_error("warm-$run", E_USER_NOTICE);
    };
    for ($i = 0; $i < 10; $i++) {
        $warm();
    }
}
trigger_error("park-$run", E_USER_WARNING);

// Read without changing it: set and put straight back.
$after = set_error_handler(null);
restore_error_handler();
echo 'HANDLER-AFTER:', $after === $handler ? 'own' : 'other', "\n";
echo 'REENTERED:', $reentered, "\n";

if ($phases) {
    // Back to the handler under it, then none over that: the depth the handler
    // was called at, with no handler and no call running.
    restore_error_handler();
    set_error_handler(null);
    // Parked in a stream wrapper's open rather than directly: fopen() takes as
    // many arguments as trigger_error() above, so stream_open()'s frame lands
    // where the handler's did, entered from C over a frame where the error was
    // raised, with the arguments a handler gets. A statement of its own: a call
    // around it would push its frame first.
    OxphpErrorHandlerParkWrapper::$park = $park;
    $stream = fopen('oxehslot://bare', 'r');
    fclose($stream);
    restore_error_handler();
    restore_error_handler();
}
echo "DONE\n";
