<?php

declare(strict_types=1);

// Shared state for the tests that end a request inside an internal function —
// usort() is the one that matters, because it sorts a copy of the array — and
// then look at whether the worker was recycled. Pulled in with require_once, so
// in worker mode it outlives a single request: the trigger, the neighbour it may
// send a request to, and the probe after it all read and write the same one.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

require_once __DIR__ . '/ini_put_back_probe.php';

if (!class_exists('OxphpBailoutLeak', false)) {
    final class OxphpBailoutLeak
    {
        /** Rows enough that what a sort of them leaves is several times the retire threshold. */
        public const BIG_ROWS = 40000;

        /** Rows few enough that what a sort of them leaves is a fraction of it. */
        public const SMALL_ROWS = 200;

        /** Rows that leave more than the threshold but less than the threshold and a full pool of fibers together. */
        public const MEDIUM_ROWS = 20000;

        /** Bytes in each row's string, which is most of what a row weighs. */
        public const ROW_BYTES = 250;

        /** The heap growth past which a worker is retired; the same number as the server's. */
        public const RETIRE_BYTES = 4 * 1024 * 1024;

        /** Requests parked at once in a burst: with the sort's and this one, close to the most a worker runs at a time. */
        public const BURST = 250;

        /** Objects in a cycle of their own, and the bytes of the string each holds. */
        public const CYCLES = 2400;
        public const CYCLE_BYTES = 2500;

        /**
         * Where a trigger leaves what its probe reads: the worker it ran on may
         * be gone by then, and its statics with it. One file per case.
         */
        public const STATE = '/tmp/oxphp-bailout-leak-state-';

        /** What the loop-and-keep trigger holds on to on purpose. */
        public static array $retained = [];

        /** Stands for the worker this is running on; a fresh worker draws a new one. */
        private static ?string $worker = null;

        public static function worker(): string
        {
            return self::$worker ??= bin2hex(random_bytes(8));
        }

        /**
         * Leaves what the probe needs to tell a worker that was retired from one
         * that was not: the worker, how many were retired on their own schedule
         * so far, and how large its heap is before the trigger adds anything.
         */
        public static function arm(string $case, array $extra = []): void
        {
            file_put_contents(self::STATE . $case, json_encode($extra + [
                'worker' => self::worker(),
                'scheduled' => oxphp_ini_put_back_scheduled_recycles(),
                'heap' => memory_get_usage(),
            ]));
        }

        /** Adds what a trigger learns part-way, to what it left for its probe. */
        public static function note(string $case, string $key, mixed $value): void
        {
            $state = self::state($case) ?? [];
            $state[$key] = $value;
            file_put_contents(self::STATE . $case, json_encode($state));
        }

        /** @return array<string, mixed>|null */
        public static function state(string $case): ?array
        {
            $raw = @file_get_contents(self::STATE . $case);
            $state = is_string($raw) ? json_decode($raw, true) : null;

            return is_array($state) ? $state : null;
        }

        /**
         * Strings that are each their own allocation, so that a copy of the
         * array holds a reference to every one of them.
         *
         * @return list<string>
         */
        public static function rows(int $count): array
        {
            $rows = [];
            for ($i = 0; $i < $count; $i++) {
                $rows[] = str_pad((string) $i, self::ROW_BYTES, 'x');
            }

            return $rows;
        }

        /**
         * Sorts the rows with a comparator that, a hundred calls in, spins until
         * the request's time limit ends it — so the request is ended inside
         * usort(), with the copy of the array it sorts still held there.
         */
        public static function sortUntilTimeout(array $rows): void
        {
            $calls = 0;
            set_time_limit(1);
            usort($rows, static function (string $a, string $b) use (&$calls): int {
                if (++$calls > 100) {
                    $spin = 0;
                    while (true) {
                        $spin++;
                    }
                }

                return $a <=> $b;
            });
        }

        /**
         * Parks BURST requests on this worker at once and sends one more that is
         * ended inside usort() over $rows rows, then waits for all of them. Every
         * one of them is a fiber the worker has to create unless it kept some from
         * before, and a created fiber keeps its stacks for as long as the worker
         * lives: that growth is the worker's own, and is no leak.
         *
         * Returns how much the heap grew over the burst, for a test to say that it
         * really did grow past the threshold — a worker that already had its
         * fibers would make a test of this pass for nothing.
         */
        public static function burst(int $rows): int
        {
            $before = memory_get_usage();
            $paths = array_fill(0, self::BURST, '/tests/fibers/fixture_bailout_parked_neighbour.php');
            $paths[] = '/tests/fibers/fixture_bailout_in_usort.php?rows=' . $rows;

            $socks = [];
            foreach ($paths as $path) {
                $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 10.0);
                if ($sock === false) {
                    throw new \RuntimeException("burst connect failed: $errstr ($errno)");
                }
                stream_set_timeout($sock, 30);
                fwrite($sock, "GET $path HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
                $socks[] = $sock;
            }

            $parked = 0;
            $sorted = 0;
            foreach ($socks as $i => $sock) {
                $raw = (string) stream_get_contents($sock);
                fclose($sock);
                if (str_contains($raw, "\r\n\r\nparked")) {
                    $parked++;
                }
                if (str_contains($raw, 'finished')) {
                    $sorted++;
                }
            }
            if ($parked !== self::BURST || $sorted !== 0) {
                throw new \RuntimeException("burst: $parked of " . self::BURST . " parked requests answered, $sorted sorts ran to their end");
            }

            return memory_get_usage() - $before;
        }

        /**
         * Objects that each refer to themselves, so that letting go of them frees
         * nothing until the cycle collector runs. Each is built through a
         * variable that is assigned the next one, and that assignment is what
         * makes it a possible root: it goes into the collector's buffer, and
         * stays there until a collection runs.
         *
         * @return list<object>
         */
        public static function cycles(int $count): array
        {
            $nodes = [];
            for ($i = 0; $i < $count; $i++) {
                $node = new \stdClass();
                $node->payload = str_pad((string) $i, self::CYCLE_BYTES, 'x');
                $node->peer = $node;
                $nodes[] = $node;
            }

            return $nodes;
        }

        /**
         * The same objects, built without a variable that ever names one: each
         * is only ever an element of the array, so nothing along the way makes it
         * a possible root and the collector's buffer does not hold it.
         *
         * @return list<object>
         */
        public static function unbufferedCycles(int $count): array
        {
            $nodes = [];
            for ($i = 0; $i < $count; $i++) {
                $nodes[$i] = new \stdClass();
                $nodes[$i]->payload = str_pad((string) $i, self::CYCLE_BYTES, 'x');
                $nodes[$i]->peer = $nodes[$i];
            }

            return $nodes;
        }

        /**
         * Holds the cycles in a local and is ended inside array_map() by the time
         * limit, so the walk after the bailout lets go of them and they are left
         * as garbage. What the trigger notes for its probe is what they weighed
         * and how many possible roots the collector held at that moment, which
         * is what tells the two kinds of cycle apart.
         */
        public static function mapUntilTimeout(string $case, bool $buffered = true): void
        {
            $nodes = $buffered ? self::cycles(self::CYCLES) : self::unbufferedCycles(self::CYCLES);
            self::note($case, 'held', memory_get_usage());
            self::note($case, 'roots', gc_status()['roots']);
            $calls = 0;
            set_time_limit(1);
            array_map(static function (int $i) use (&$calls, $nodes): int {
                if (++$calls > 100) {
                    $spin = 0;
                    while (true) {
                        $spin++;
                    }
                }

                return $i;
            }, range(1, 1000));
        }

        /** What the probe says about the worker it finds itself on, read once. */
        public static function assertRetired(TestCase $t, string $case): void
        {
            $state = self::state($case);
            $t->assertTrue('the trigger left its state', $state !== null);
            $t->assertNotNull('the scheduled recycles were read before', $state['scheduled'] ?? null);

            // The recycle is counted as the worker's thread is reaped, which can
            // come a moment after this request is taken by its replacement.
            $after = null;
            $deadline = microtime(true) + 5.0;
            do {
                $after = oxphp_ini_put_back_scheduled_recycles();
                if ($after !== null && $after > ($state['scheduled'] ?? PHP_INT_MAX)) {
                    break;
                }
                usleep(50_000);
            } while (microtime(true) < $deadline);

            $held = memory_get_usage() - ($state['heap'] ?? 0);
            $t->assertSame(
                "the worker it ran on retired (this one holds $held bytes more than before the trigger)",
                $after,
                ($state['scheduled'] ?? 0) + 1
            );
            $t->assertTrue('and this request runs on the worker that replaced it', self::worker() !== ($state['worker'] ?? ''));
        }

        /** The same, for a trigger whose worker must have been left alone. */
        public static function assertKept(TestCase $t, string $case): void
        {
            $state = self::state($case);
            $t->assertTrue('the trigger left its state', $state !== null);
            $t->assertSame('this request runs on the worker the trigger ran on', self::worker(), $state['worker'] ?? null);
            $t->assertNotNull('the scheduled recycles were read before', $state['scheduled'] ?? null);
            $t->assertSame(
                'and no worker was retired on its own schedule in between',
                oxphp_ini_put_back_scheduled_recycles(),
                $state['scheduled'] ?? null
            );
        }
    }
}
