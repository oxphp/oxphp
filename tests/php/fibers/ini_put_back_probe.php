<?php

declare(strict_types=1);

// Shared state for fibers/test_ini_put_back_does_not_park, the fixture it sends
// a neighbour request to, and the probe after it. Pulled in with require_once,
// so in worker mode it outlives a single request and all three read and write
// the same one.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!class_exists('OxphpIniPutBackProbe', false)) {
    final class OxphpIniPutBackProbe
    {
        /** Whether the destructor run by putting the directive back ran at all. */
        public static bool $destructorRan = false;

        /** Up while that destructor is sleeping. */
        public static bool $inDestructor = false;

        /** Whether the neighbour request has run. */
        public static bool $neighbourRan = false;

        /** What the neighbour found $inDestructor to be when it ran. */
        public static ?bool $neighbourSawDestructor = null;

        /** Whether the throwing destructor test_ini_put_back_reports_a_throw leaves ran. */
        public static bool $throwerRan = false;

        /** Whether the destructor test_ini_put_back_survives_a_growing_set leaves ran. */
        public static bool $growerRan = false;

        /**
         * Directives that destructor sets: all changeable from a script, none
         * touched by the request it runs in, and more than the set of modified
         * directives starts with room for, so it has to grow.
         */
        public const GROWN = [
            'highlight.comment' => '#000001',
            'highlight.default' => '#000002',
            'highlight.html' => '#000003',
            'highlight.keyword' => '#000004',
            'highlight.string' => '#000005',
            'precision' => '7',
            'serialize_precision' => '9',
            'default_socket_timeout' => '17',
            'user_agent' => 'oxphp-grown',
            'arg_separator.output' => ';',
            'from' => 'grown@example.invalid',
            'docref_root' => '/grown/',
            'docref_ext' => '.grown',
            'error_prepend_string' => '<grown>',
            'error_append_string' => '</grown>',
            'unserialize_max_depth' => '77',
            'pcre.backtrack_limit' => '777777',
            'pcre.recursion_limit' => '77777',
            'url_rewriter.tags' => 'a=href',
            'default_charset' => 'ISO-8859-1',
        ];

        /** Strings that destructor allocates over the storage the set gave up. */
        public static array $fill = [];

        /**
         * One length per allocator size class from 320 to 3072 bytes, the
         * classes the set's storage passes through as it grows from 8 entries
         * to 64: a string of that length plus its header lands in that class.
         */
        public const FILL_LENGTHS = [290, 350, 420, 480, 600, 730, 860, 990, 1250, 1500, 1760, 2000, 2500, 3000];

        /**
         * Written by the task test_ini_put_back_survives_a_growing_set leaves
         * behind, once it is done. A file, not a property: the task outlives
         * the request that left it.
         */
        public const GROWER_TASK_DONE = '/tmp/oxphp-ini-put-back-grower-task-done';

        /**
         * Where test_ini_put_back_fatal_retires_the_worker leaves what its probe
         * reads: the worker it ran on is gone by then, and its statics with it.
         */
        public const FATAL_STATE = '/tmp/oxphp-ini-put-back-fatal-state';

        /** The neighbour's connection, held open so it is not answered as closed. */
        public static mixed $sock = null;
    }
}

if (!function_exists('oxphp_ini_put_back_scheduled_recycles')) {
    /**
     * How many workers have been recycled on their own schedule, or null when
     * /metrics is unreachable or carries no worker-mode block. An absent
     * reason="scheduled" line is a zero; see breaker/breaker_probe.php.
     */
    function oxphp_ini_put_back_scheduled_recycles(): ?int
    {
        $ctx = stream_context_create(['http' => ['timeout' => 3.0]]);
        $body = @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
        if (!is_string($body) || !preg_match('/^oxphp_worker_recycles_total \d+$/m', $body)) {
            return null;
        }
        if (preg_match('/^oxphp_worker_recycles_by_reason_total\{reason="scheduled"\} (\d+)$/m', $body, $m)) {
            return (int) $m[1];
        }

        return 0;
    }
}
