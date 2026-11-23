<?php

declare(strict_types=1);

// The guard Symfony HttpClient's CurlHttpClient (8.1.11, CurlResponse::perform())
// keeps around curl_multi_exec(), on plain PHP: one client, and so one multi
// handle, that every request on the worker shares, with a flag raised for the
// whole of perform(). A request that finds the flag raised fails its response,
// as Symfony's does with "Userland callback cannot use the client nor the
// response while processing …". So a request suspended inside perform() is
// another request's failure. Not a test itself, so no suite lists it.

if (!class_exists('OxphpSharedCurlClient', false)) {
    final class OxphpSharedCurlClient
    {
        public \CurlMultiHandle $mh;
        public bool $performing = false;
        /** @var list<array{string, float}> who found the flag raised, and when */
        public array $reentries = [];
        /** @var list<array{string, float, float}> who ran perform(), from, to */
        public array $performs = [];
        /** @var list<array{string, int, int, float, mixed}> who, code, $still_running, when, probe */
        public array $execs = [];
        /** @var list<array{string, float, float, int}> who, from, to, what curl_multi_select() returned */
        public array $selects = [];
        /** @var array<int, int> spl_object_id() of a finished handle => its CURLE_* code */
        public array $done = [];
        /** Read as each curl_multi_exec() returns, into `execs`. */
        public ?\Closure $probe = null;

        public function __construct()
        {
            $this->mh = curl_multi_init();
        }

        public function perform(string $who): bool
        {
            if ($this->performing) {
                $this->reentries[] = [$who, microtime(true)];
                return false;
            }
            $this->performing = true;
            $started = microtime(true);
            try {
                do {
                    $running = 0;
                    $code = curl_multi_exec($this->mh, $running);
                    $this->execs[] = [$who, $code, $running, microtime(true), $this->probe ? ($this->probe)() : null];
                } while ($code === CURLM_CALL_MULTI_PERFORM);
                while ($info = curl_multi_info_read($this->mh)) {
                    if ($info['msg'] === CURLMSG_DONE) {
                        $this->done[spl_object_id($info['handle'])] = $info['result'];
                    }
                }
            } finally {
                $this->performing = false;
                $this->performs[] = [$who, $started, microtime(true)];
            }
            return true;
        }

        /** Drives the transfer of $ch to its end as Symfony's stream loop does,
         *  perform() then select(), whichever request's perform() finds it done.
         *  Returns null, or why the transfer failed. $inSelect is told when each
         *  select starts and ends. */
        public function run(string $who, \CurlHandle $ch, ?\Closure $inSelect = null, float $for = 8.0): ?string
        {
            $id = spl_object_id($ch);
            $deadline = microtime(true) + $for;
            while (microtime(true) < $deadline) {
                if (!$this->perform($who)) {
                    return 'found perform() under way';
                }
                if (isset($this->done[$id])) {
                    return $this->done[$id] === CURLE_OK ? null : 'cURL error ' . $this->done[$id];
                }
                $started = microtime(true);
                $inSelect?->__invoke(true);
                try {
                    $selected = curl_multi_select($this->mh, 1.0);
                } finally {
                    $inSelect?->__invoke(false);
                }
                $this->selects[] = [$who, $started, microtime(true), $selected];
            }
            return 'timed out';
        }
    }
}
