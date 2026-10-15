<?php

declare(strict_types=1);

// Driven by worker/test_shared_pool_idle_eviction. Parks slots in a pool with a
// short idle timeout and returns, which leaves the worker that ran it with
// nothing to do but wait for its next request. Answers with that worker's id.

use OxPHP\Server\Worker;
use OxPHP\Shared\Counter;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Declared conditionally: worker mode runs this file again on every request,
// and a class it declared stays declared.
if (!class_exists('OxphpPoolIdleKept', false)) {
    final class OxphpPoolIdleKept
    {
        /** @var list<string> */
        public static array $kept = [];
    }
}

$run = (string) ($_GET['run'] ?? '');
$case = (string) ($_GET['case'] ?? '');

switch ($case) {
    case 'idle':
        $calls = Registry::counter("pool-idle-calls-$run", static fn (): Counter => new Counter());
        $p = Registry::pool("pool-idle-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($calls): void {
                $calls->add();
            },
            maxSize: 1,
            idleTimeoutMs: 1000,
        ));
        $p->acquire()->release();
        break;

    case 'echo':
        // $destroy writes output while the worker has no request in flight —
        // straight out, and into a buffer it leaves open by throwing before it
        // ends it — and registers a shutdown function.
        $calls = Registry::counter("pool-idle-echo-calls-$run", static fn (): Counter => new Counter());
        $p = Registry::pool("pool-idle-echo-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($calls, $run): void {
                echo "LEAK-$run-plain\n";
                register_shutdown_function(static function () use ($run): void {
                    echo "LEAK-$run-shutdown\n";
                });
                $calls->add();
                ob_start();
                echo "LEAK-$run-buffered\n";
                throw new \RuntimeException('closing the connection failed');
            },
            maxSize: 1,
            idleTimeoutMs: 1000,
        ));
        $p->acquire()->release();
        break;

    case 'released':
        // $destroy leaves two buffers open, each with an object as its callback.
        // Releasing the upper one opens a third buffer from its destructor, which
        // is released in turn, callback and all; the lower one, released last,
        // looks at the output state from its own: by then every buffer is gone,
        // so none may still be reported as active.
        $calls = Registry::counter("pool-idle-released-calls-$run", static fn (): Counter => new Counter());
        $stale = Registry::counter("pool-idle-released-stale-$run", static fn (): Counter => new Counter());
        $started = Registry::counter("pool-idle-released-started-$run", static fn (): Counter => new Counter());
        $p = Registry::pool("pool-idle-released-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($calls, $stale, $started): void {
                ob_start(new class ($stale) {
                    public function __construct(private Counter $stale)
                    {
                    }

                    public function __invoke(string $buffer): string
                    {
                        return $buffer;
                    }

                    public function __destruct()
                    {
                        if (ob_get_level() === 0 && ob_get_status() !== []) {
                            $this->stale->add();
                        }
                    }
                });
                ob_start(new class ($started) {
                    public function __construct(private Counter $started)
                    {
                    }

                    public function __invoke(string $buffer): string
                    {
                        return $buffer;
                    }

                    public function __destruct()
                    {
                        ob_start(new class ($this->started) {
                            public function __construct(private Counter $started)
                            {
                            }

                            public function __invoke(string $buffer): string
                            {
                                return $buffer;
                            }

                            public function __destruct()
                            {
                                $this->started->add();
                            }
                        });
                    }
                });
                $calls->add();
            },
            maxSize: 1,
            idleTimeoutMs: 1000,
        ));
        $p->acquire()->release();
        break;

    case 'late':
        // Two pools whose slots go stale on separate passes of the eviction.
        // The first $destroy leaves behind what runs PHP only as the eviction
        // gives it up, once the callback has returned: a shutdown function
        // whose argument throws from its destructor, one whose argument
        // registers another from its own, and a buffer whose callback registers
        // a shutdown function from its own. The second $destroy runs on the
        // next pass, with no request between the two.
        $calls = Registry::counter("pool-idle-late-calls-$run", static fn (): Counter => new Counter());
        $freed = Registry::counter("pool-idle-late-freed-$run", static fn (): Counter => new Counter());
        $nested = Registry::counter("pool-idle-late-nested-$run", static fn (): Counter => new Counter());
        $first = Registry::pool("pool-idle-late-first-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($freed, $nested): void {
                register_shutdown_function(static function (object $conn): void {
                }, new class () {
                    public function __destruct()
                    {
                        throw new \RuntimeException('closing the connection failed');
                    }
                });
                register_shutdown_function(static function (object $conn): void {
                }, new class ($nested) {
                    public function __construct(private Counter $nested)
                    {
                    }

                    public function __destruct()
                    {
                        register_shutdown_function(static function (object $conn): void {
                        }, new class ($this->nested) {
                            public function __construct(private Counter $nested)
                            {
                            }

                            public function __destruct()
                            {
                                $this->nested->add();
                            }
                        });
                    }
                });
                ob_start(new class ($freed) {
                    public function __construct(private Counter $freed)
                    {
                    }

                    public function __invoke(string $buffer): string
                    {
                        return $buffer;
                    }

                    public function __destruct()
                    {
                        register_shutdown_function(static function (object $conn): void {
                        }, new class ($this->freed) {
                            public function __construct(private Counter $freed)
                            {
                            }

                            public function __destruct()
                            {
                                $this->freed->add();
                            }
                        });
                    }
                });
            },
            maxSize: 1,
            idleTimeoutMs: 1000,
        ));
        $second = Registry::pool("pool-idle-late-second-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($calls): void {
                $calls->add();
            },
            maxSize: 1,
            idleTimeoutMs: 2000,
        ));
        $first->acquire()->release();
        $second->acquire()->release();
        break;

    case 'heap':
        // $destroy keeps more than the worker's leak threshold on purpose, the
        // way a teardown might fill a cache, in a static that outlives the call.
        $calls = Registry::counter("pool-idle-heap-calls-$run", static fn (): Counter => new Counter());
        $p = Registry::pool("pool-idle-heap-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($calls): void {
                OxphpPoolIdleKept::$kept[] = str_repeat('x', 8 * 1024 * 1024);
                $calls->add();
            },
            maxSize: 1,
            idleTimeoutMs: 1000,
        ));
        $p->acquire()->release();
        break;

    case 'sort':
        // Ended by its time limit inside usort(), over rows too few for what the
        // sort leaves behind to come near the threshold. Answers 504.
        $rows = [];
        for ($i = 0; $i < 200; $i++) {
            $rows[] = str_pad((string) $i, 250, 'x');
        }
        $compared = 0;
        set_time_limit(1);
        usort($rows, static function (string $a, string $b) use (&$compared): int {
            if (++$compared > 100) {
                $spin = 0;
                while (true) {
                    $spin++;
                }
            }

            return $a <=> $b;
        });
        break;

    case 'fatal':
        // Two idle slots that go stale together. $destroy fatals on the older
        // one, which the eviction reaches first, and only on it. A compile error
        // rather than E_USER_ERROR: an error handler an earlier request left on
        // this worker would turn that into an exception, and no handler sees
        // E_COMPILE_ERROR. The compile fails before the class is bound, so
        // nothing is declared.
        $calls = Registry::counter("pool-idle-fatal-calls-$run", static fn (): Counter => new Counter());
        $p = Registry::pool("pool-idle-fatal-$run", static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => new \stdClass(),
            destroy: static function (\stdClass $r) use ($calls): void {
                $calls->add();
                if ($r->fatal ?? false) {
                    eval('abstract class OxphpPoolIdleFatal { abstract public function f() {} }');
                }
            },
            maxSize: 2,
            idleTimeoutMs: 1000,
        ));
        $older = $p->tryAcquire();
        $newer = $p->tryAcquire();
        $older->get()->fatal = true;
        $older->release();
        $newer->release();
        break;

    case 'none':
        // A Shared call: a bailout held back and never raised again on this
        // thread would end this request with it.
        Registry::counter("pool-idle-fatal-calls-$run", static fn (): Counter => new Counter())->get();
        break;

    default:
        http_response_code(400);
        echo "FAIL: unknown case\n";
        exit;
}

echo 'OK worker=' . Worker::current()->id() . "\n";
