<?php

declare(strict_types=1);

// Shared state for the fibers/test_*error_mode* tests and the requests and tasks
// they start. Pulled in with require_once, so in worker mode it outlives a single
// request: the test and the requests it sends read and write the same one.
//
// `new SplFileObject()` and `new DirectoryIterator()` run their open in the mode
// PHP turns a warning into an exception in (EH_THROW), and restore the previous
// mode when the open returns. Opening a path of the wrapper below runs the
// wrapper's code inside that window, so a request or task can park there, or
// fatal there.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

require_once __DIR__ . '/../engine_fatal.php';

if (!class_exists('OxphpErrorModeProbe', false)) {
    final class OxphpErrorModeProbe
    {
        /** @var array<string, bool> Up while the opener with that tag is parked inside its window. */
        public static array $inWindow = [];

        /** @var array<string, bool> Set by the opener with that tag just before it fatals. */
        public static array $fataling = [];

        /** @var array<string, resource|false> Requests a fataling opener sent, kept for the test to read. */
        public static array $sent = [];

        /** Sends one request for a fixture of this directory to this server and returns its socket, or false. */
        public static function send(string $query, string $fixture = 'fixture_error_mode.php')
        {
            $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
            if ($sock === false) {
                return false;
            }
            stream_set_timeout($sock, 10);
            fwrite($sock, "GET /tests/fibers/$fixture?$query HTTP/1.0\r\n"
                . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

            return $sock;
        }

        /** Reads a response to its end. Hooked: parks the caller until it is all there. */
        public static function read($sock): string
        {
            if ($sock === false || $sock === null) {
                return '';
            }
            $resp = (string) stream_get_contents($sock);
            fclose($sock);

            return $resp;
        }

        public static function register(): void
        {
            if (!in_array('oxehpark', stream_get_wrappers(), true)) {
                stream_wrapper_register('oxehpark', OxphpErrorModeWrapper::class);
            }
        }

        /**
         * Opens `oxehpark://park/<ms>/<tag>` as a file or `oxehpark://fatal/<tag>`
         * as a directory, and says how the open ended: the class and message of
         * what it threw.
         *
         * The fatal goes through DirectoryIterator, which opens in the same mode:
         * an SplFileObject holds its path without a reference of its own until
         * its open returns, so a fatal inside that open leaves the object and the
         * call that passed the path both releasing it.
         */
        public static function open(string $path): string
        {
            self::register();
            try {
                if (str_starts_with($path, 'oxehpark://fatal/')) {
                    new \DirectoryIterator($path);
                } else {
                    new \SplFileObject($path);
                }

                return 'opened';
            } catch (\Throwable $e) {
                return get_class($e) . ':' . $e->getMessage();
            }
        }

        /**
         * Raises a notice and a warning under an error handler of its own, and
         * reports what that handler heard, what was thrown instead, and what was
         * printed instead. In the mode a caller's own code runs in, the handler
         * hears both and nothing is thrown or printed.
         *
         * @return array{heard: list<string>, thrown: string, shown: string}
         */
        public static function measure(string $tag): array
        {
            $heard = [];
            set_error_handler(static function (int $severity, string $message) use (&$heard): bool {
                $heard[] = $message;

                return true;
            });
            ob_start();
            try {
                trigger_error("notice-$tag", E_USER_NOTICE);
                trigger_error("warning-$tag", E_USER_WARNING);
                $thrown = 'none';
            } catch (\Throwable $e) {
                $thrown = get_class($e) . ':' . $e->getMessage();
            }
            $shown = (string) ob_get_clean();
            restore_error_handler();

            return ['heard' => $heard, 'thrown' => $thrown, 'shown' => $shown];
        }
    }

    /** Sets OxphpErrorModeProbe::$inWindow[$key] once the last reference to it goes. */
    final class OxphpErrorModeWrapper
    {
        /** @var resource|null Set by PHP on every wrapper instance. */
        public $context;

        public function url_stat(string $path, int $flags): array|false
        {
            return false;
        }

        public function dir_opendir(string $path, int $options): bool
        {
            OxphpErrorModeProbe::$fataling[substr($path, strlen('oxehpark://fatal/'))] = true;
            engine_fatal('OxphpErrorModeFatal');
        }

        public function stream_open(string $path, string $mode, int $options, ?string &$openedPath): bool
        {
            [, $ms, $tag] = explode('/', substr($path, strlen('oxehpark://')));

            OxphpErrorModeProbe::$inWindow[$tag] = true;
            // Hooked: parks the request or task inside the open's window.
            usleep((int) $ms * 1000);
            OxphpErrorModeProbe::$inWindow[$tag] = false;

            // In the opener's own mode this becomes the RuntimeException the
            // open throws; in any other, a warning and the open's own message.
            trigger_error("after-park-$tag", E_USER_WARNING);

            return false;
        }
    }
}
