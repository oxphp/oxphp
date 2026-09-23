<?php

declare(strict_types=1);

// Shared state for fibers/test_session_written_after_flush_throw and the probe
// after it. Pulled in with require_once, so in worker mode it outlives a single
// request and both read and write the same one.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpSessionFlushThrowProbe', false)) {
    final class OxphpSessionFlushThrowProbe
    {
        /** Whether the save handler's write() ran, and with what. */
        public static ?string $written = null;

        /** Whether the save handler's close() ran. */
        public static bool $closed = false;

        /** Whether the throwing output callback ran. */
        public static bool $callbackRan = false;

        /** Whether it had already run when write() did. */
        public static ?bool $callbackRanBeforeWrite = null;
    }

    final class OxphpSessionFlushThrowHandler implements SessionHandlerInterface
    {
        public function open(string $path, string $name): bool
        {
            return true;
        }

        public function close(): bool
        {
            OxphpSessionFlushThrowProbe::$closed = true;
            return true;
        }

        public function read(string $id): string
        {
            return '';
        }

        public function write(string $id, string $data): bool
        {
            OxphpSessionFlushThrowProbe::$written = $data;
            OxphpSessionFlushThrowProbe::$callbackRanBeforeWrite = OxphpSessionFlushThrowProbe::$callbackRan;
            return true;
        }

        public function destroy(string $id): bool
        {
            return true;
        }

        public function gc(int $max_lifetime): int
        {
            return 0;
        }
    }
}
