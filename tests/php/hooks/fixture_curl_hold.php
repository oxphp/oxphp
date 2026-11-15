<?php

declare(strict_types=1);

// Parks a request fiber inside a cURL transfer on a handle the whole worker shares,
// so the request that fires this one can call cURL on the same handle while the
// transfer is still waiting for its answer. Answers with JSON: what the transfer
// got, and when it finished.
//
//   ?mode=easy   curl_exec() on a CurlHandle kept in $sharedState['curl_ch']
//   ?mode=multi  a curl_multi_exec()/curl_multi_select() loop on a CurlMultiHandle
//                kept in $sharedState['curl_mh'], its easy handle in
//                $sharedState['curl_ch']; it runs until every handle in it is done,
//                including any the other request adds, and then takes its own out
//   ?mode=share  as easy, with a CurlShareHandle kept in $sharedState['curl_sh']
//   ?mode=member adds the CurlHandle to a CurlMultiHandle and does not drive it: the
//                other request decides what becomes of the transfer. `?how=wait`
//                (the default) waits for $sharedState['curl_release'], calls
//                curl_setopt() on the handle itself and then takes it out; `?how=drop` frees the multi handle first and
//                then waits; `?how=end` returns at once, the handle still in
//   ?mode=adder  waits for $sharedState['curl_mh'], adds a transfer of its own to it,
//                raising $sharedState['curl_adder_waiting'] just before, and drives
//                the multi handle until that transfer is done
//   ?mode=reuse  frees a CurlMultiHandle with two easy handles in it, the second
//                with a callback holding the only reference to an object whose
//                destructor, run once the first handle is freed, builds an easy
//                handle and keeps it in $sharedState['curl_ch']. `?then=park` (the
//                default) stays in that destructor for 1 s; `?then=add` first makes
//                an object that takes the freed handle's number, adds the built
//                handle to $sharedState['curl_mh'] and takes it out 1 s later
//
// `?delay=` is the milliseconds the peer waits before it answers, 2000 by default.
// `?nested=1` (easy mode) has the transfer's progress callback call curl_setopt()
// on the transfer's own handle once, from inside curl_exec(), and raise
// $sharedState['curl_nested'] when it has. `?inner=multi` (easy mode) has the
// progress callback, on its first call, run a curl_multi_exec()/curl_multi_select()
// loop of its own until a 2 s transfer is done, on a CurlMultiHandle kept in
// $sharedState['curl_mh'], raising $sharedState['curl_in_select'] as multi mode
// does and reporting its selects in `selects`. `?yield=1` (multi mode) suspends the
// loop for 20 ms after each curl_multi_select(), and `?yield=exec` after each
// curl_multi_exec(), as an HTTP client does while it runs its promise callbacks.
// `?select=` (multi mode) is the
// timeout the loop passes curl_multi_select(), 1.0 s by default; the loop raises
// $sharedState['curl_in_select'] for the length of each call, with the time it
// began in $sharedState['curl_select_started'], and reports each one as
// [started, ended, returned] in `selects`. `?fiber=1` (multi mode) makes
// each curl_multi_exec() inside a userland Fiber, where the fiber cannot be
// suspended. `?cb=1` (member mode) has the
// progress callback call curl_setopt() on its handle once, in whichever request
// drives the transfer, and store how long that took in $sharedState['curl_cb_took'].
// `?warned=1` (member mode) adds the handle under an exception: an error handler
// throws for the warning ext/curl raises while it adds it. `?readd=1` (member mode,
// `how=wait`) adds a new easy handle to the multi handle right after taking the
// first one out, keeps it in for 1.5 s and records when it took that one out.
$peer = getenv('NET_PEER') ?: 'hooks-netpeer';
$mode = (string) ($_GET['mode'] ?? 'easy');
$delay = (int) ($_GET['delay'] ?? 2000);
$url = "http://{$peer}:8080/{$delay}/" . ($mode === 'adder' ? 'adder' : 'holder');
$result = ['mode' => $mode];

try {
    $ch = curl_init($url);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);

    if ($mode === 'multi') {
        $mh = curl_multi_init();
        curl_multi_add_handle($mh, $ch);
        $sharedState['curl_mh'] = $mh;
        $sharedState['curl_ch'] = $ch;
        // Set before the loop, so the request that acts on the handle can tell "the
        // holder is about to park" from "the holder has not got here yet".
        $sharedState['curl_parked'] = true;

        $yield = (string) ($_GET['yield'] ?? '');
        $inFiber = ($_GET['fiber'] ?? '') === '1';
        $selectTimeout = (float) ($_GET['select'] ?? 1.0);
        $selects = [];
        $codes = [];
        $deadline = microtime(true) + 8.0;
        do {
            if ($inFiber) {
                $fiber = new \Fiber(static function () use ($mh, &$running): int {
                    return curl_multi_exec($mh, $running);
                });
                $fiber->start();
                $codes[$fiber->getReturn()] = true;
            } else {
                $codes[curl_multi_exec($mh, $running)] = true;
            }
            if ($running > 0) {
                if ($yield === 'exec') {
                    usleep(20_000);
                }
                $selectStarted = microtime(true);
                $sharedState['curl_select_started'] = $selectStarted;
                $sharedState['curl_in_select'] = true;
                try {
                    $selected = curl_multi_select($mh, $selectTimeout);
                } finally {
                    $sharedState['curl_in_select'] = false;
                }
                $selects[] = [$selectStarted, microtime(true), $selected];
                if ($yield === '1') {
                    usleep(20_000);
                }
            }
        } while ($running > 0 && microtime(true) < $deadline);
        $result['finished'] = microtime(true);
        $result['body'] = curl_multi_getcontent($ch);
        $result['codes'] = array_keys($codes);
        $result['selects'] = $selects;
        $result['removed'] = curl_multi_remove_handle($mh, $ch);
        $result['removed_at'] = microtime(true);
    } elseif ($mode === 'adder') {
        $deadline = microtime(true) + 3.0;
        while (!isset($sharedState['curl_mh']) && microtime(true) < $deadline) {
            usleep(20_000);
        }
        $mh = $sharedState['curl_mh'];
        $started = microtime(true);
        $sharedState['curl_adder_waiting'] = $started;
        $result['added'] = curl_multi_add_handle($mh, $ch);
        $result['add_took'] = microtime(true) - $started;

        $codes = [];
        $deadline = microtime(true) + 8.0;
        while (microtime(true) < $deadline) {
            $codes[curl_multi_exec($mh, $running)] = true;
            if (curl_multi_getcontent($ch) === "curl-done:/{$delay}/adder") {
                break;
            }
            curl_multi_select($mh, 0.5);
        }
        $result['body'] = curl_multi_getcontent($ch);
        $result['codes'] = array_keys($codes);
        $result['removed'] = curl_multi_remove_handle($mh, $ch);
    } elseif ($mode === 'member') {
        $how = (string) ($_GET['how'] ?? 'wait');
        if (($_GET['cb'] ?? '') === '1') {
            $called = false;
            curl_setopt_array($ch, [
                CURLOPT_NOPROGRESS => false,
                CURLOPT_XFERINFOFUNCTION => static function ($handle) use (&$called, &$sharedState): int {
                    if (!$called) {
                        $called = true;
                        $started = microtime(true);
                        curl_setopt($handle, CURLOPT_USERAGENT, 'callback');
                        $sharedState['curl_cb_took'] = microtime(true) - $started;
                    }
                    return 0;
                },
            ]);
        }
        $mh = curl_multi_init();
        if (($_GET['warned'] ?? '') === '1') {
            // ext/curl warns that the CURLOPT_FILE stream is gone while it adds the
            // handle, and adds it all the same under the exception this handler
            // makes of the warning; adding it again then says it is in already.
            $file = fopen('php://memory', 'w+');
            curl_setopt($ch, CURLOPT_FILE, $file);
            fclose($file);
            set_error_handler(static function (int $no, string $message): never {
                throw new \ErrorException($message, 0, $no);
            });
            try {
                $result['added'] = curl_multi_add_handle($mh, $ch);
            } catch (\Throwable $e) {
                $result['add_threw'] = $e->getMessage();
            } finally {
                restore_error_handler();
            }
            $result['added_again'] = curl_multi_add_handle($mh, $ch);
        } else {
            $result['added'] = curl_multi_add_handle($mh, $ch);
        }
        $sharedState['curl_ch'] = $ch;
        if ($how === 'drop') {
            // The only reference: freeing it takes the easy handle out.
            unset($mh);
        } else {
            $sharedState['curl_mh'] = $mh;
        }
        $sharedState['curl_parked'] = true;

        if ($how !== 'end') {
            $deadline = microtime(true) + 6.0;
            while (!($sharedState['curl_release'] ?? false) && microtime(true) < $deadline) {
                usleep(20_000);
            }
            $result['released'] = (bool) ($sharedState['curl_release'] ?? false);
            if ($how === 'wait') {
                $started = microtime(true);
                $result['own_setopt'] = curl_setopt($ch, CURLOPT_USERAGENT, 'holder');
                $result['own_setopt_took'] = microtime(true) - $started;
                $readd = ($_GET['readd'] ?? '') === '1';
                if ($readd) {
                    // Made before the removal, so that nothing is allocated between
                    // taking one handle out and putting the next one in.
                    $next = curl_init($url);
                    curl_setopt_array($next, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
                }
                $result['removed'] = curl_multi_remove_handle($mh, $ch);
                $result['removed_at'] = microtime(true);
                if ($readd) {
                    $result['readded'] = curl_multi_add_handle($mh, $next);
                    usleep(1_500_000);
                    curl_multi_remove_handle($mh, $next);
                    $result['readded_out_at'] = microtime(true);
                }
            }
        }
    } elseif ($mode === 'reuse') {
        $then = (string) ($_GET['then'] ?? 'park');
        $mh = curl_multi_init();
        $sharedState['curl_mh'] = $mh;
        $build = static function () use ($then, $url, $mh, &$sharedState, &$result): void {
            $placeholder = $then === 'add' ? new \stdClass() : null;
            $built = curl_init($url);
            curl_setopt_array($built, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
            $result['built_id'] = spl_object_id($built);
            $result['placeholder_id'] = $placeholder === null ? null : spl_object_id($placeholder);
            $sharedState['curl_ch'] = $built;
            if ($then === 'add') {
                $result['added'] = curl_multi_add_handle($mh, $built);
                return;
            }
            $sharedState['curl_parked'] = true;
            usleep(1_000_000);
            $result['left_at'] = microtime(true);
        };
        $keeper = new class ($build) {
            public function __construct(private \Closure $build)
            {
            }

            public function __destruct()
            {
                ($this->build)();
            }
        };
        $second = curl_init($url);
        curl_setopt($second, CURLOPT_XFERINFOFUNCTION, static function () use ($keeper): int {
            return 0;
        });
        unset($keeper);
        $result['first_id'] = spl_object_id($ch);
        $freed = curl_multi_init();
        // ext/curl frees them in the order they went in.
        curl_multi_add_handle($freed, $ch);
        curl_multi_add_handle($freed, $second);
        unset($ch, $second);
        unset($freed);
        if ($then === 'add') {
            $sharedState['curl_parked'] = true;
            usleep(1_000_000);
            $result['removed'] = curl_multi_remove_handle($mh, $sharedState['curl_ch']);
            $result['removed_at'] = microtime(true);
        }
    } else {
        if ($mode === 'share') {
            $sh = curl_share_init();
            curl_share_setopt($sh, CURLSHOPT_SHARE, CURL_LOCK_DATA_DNS);
            curl_share_setopt($sh, CURLSHOPT_SHARE, CURL_LOCK_DATA_CONNECT);
            curl_setopt($ch, CURLOPT_SHARE, $sh);
            $sharedState['curl_sh'] = $sh;
        }
        if (($_GET['inner'] ?? '') === 'multi') {
            $inner = false;
            curl_setopt_array($ch, [
                CURLOPT_NOPROGRESS => false,
                CURLOPT_XFERINFOFUNCTION => static function () use (&$inner, &$sharedState, &$result, $peer): int {
                    if ($inner) {
                        return 0;
                    }
                    $inner = true;
                    $mh = curl_multi_init();
                    $th = curl_init("http://{$peer}:8080/2000/inner");
                    curl_setopt_array($th, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
                    curl_multi_add_handle($mh, $th);
                    $sharedState['curl_mh'] = $mh;
                    $selects = [];
                    $deadline = microtime(true) + 6.0;
                    do {
                        curl_multi_exec($mh, $running);
                        if ($running > 0) {
                            $selectStarted = microtime(true);
                            $sharedState['curl_select_started'] = $selectStarted;
                            $sharedState['curl_in_select'] = true;
                            try {
                                $selected = curl_multi_select($mh, 1.0);
                            } finally {
                                $sharedState['curl_in_select'] = false;
                            }
                            $selects[] = [$selectStarted, microtime(true), $selected];
                        }
                    } while ($running > 0 && microtime(true) < $deadline);
                    $result['inner_body'] = curl_multi_getcontent($th);
                    $result['selects'] = $selects;
                    curl_multi_remove_handle($mh, $th);
                    return 0;
                },
            ]);
        }
        if (($_GET['nested'] ?? '') === '1') {
            $nested = false;
            curl_setopt_array($ch, [
                CURLOPT_NOPROGRESS => false,
                CURLOPT_XFERINFOFUNCTION => static function ($handle) use (&$nested, &$sharedState): int {
                    if (!$nested) {
                        $nested = true;
                        curl_setopt($handle, CURLOPT_USERAGENT, 'nested');
                        $sharedState['curl_nested'] = true;
                    }
                    return 0;
                },
            ]);
        }
        $sharedState['curl_ch'] = $ch;
        $sharedState['curl_parked'] = true;

        $body = curl_exec($ch);
        $result['finished'] = microtime(true);
        $result['body'] = $body;
        $result['errno'] = curl_errno($ch);
        $result['error'] = curl_error($ch);
    }
} catch (\Throwable $e) {
    $result['threw'] = get_class($e) . ': ' . $e->getMessage();
}

$sharedState['curl_parked'] = false;
echo json_encode($result);
