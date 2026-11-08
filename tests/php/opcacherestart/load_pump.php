<?php

declare(strict_types=1);

// Run with the PHP CLI, detached from the test that starts it (ox_start_load):
// keeps a fixed number of requests to this server in flight until told to stop
// or until it times out. A test request cannot do this itself: a request that
// runs with the cache holds an OPcache restart for as long as it lasts.
//
// php load_pump.php <path> <parallel> <spread> <seconds> <marker base>
// <base>.started when the requests are out, <base>.finished with the tally once
// the load is over; <base>.stop ends it early.

[, $path, $parallel, $spread, $seconds, $base] = $argv;
$deadline = microtime(true) + (float) $seconds;

$mh = curl_multi_init();
$send = static function () use ($mh, $path): void {
    $h = curl_init('http://127.0.0.1' . $path);
    curl_setopt_array($h, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 20]);
    curl_multi_add_handle($mh, $h);
};

// Spread over <spread>, the time one request parks for: sent together they would
// also end together, and a thread would be idle between one wave and the next.
$out = 0;
$nextSend = microtime(true);
$spacing = (float) $spread / (int) $parallel;

$ok = 0;
$failed = [];
while (microtime(true) < $deadline && !file_exists("$base.stop")) {
    if ($out < (int) $parallel && microtime(true) >= $nextSend) {
        $send();
        $out++;
        $nextSend += $spacing;
        if ($out === (int) $parallel) {
            file_put_contents("$base.started", '1');
        }
    }
    curl_multi_exec($mh, $running);
    curl_multi_select($mh, 0.002);
    while (($done = curl_multi_info_read($mh)) !== false) {
        $code = curl_getinfo($done['handle'], CURLINFO_RESPONSE_CODE);
        if ($code === 200) {
            $ok++;
        } else {
            $failed[$code] = ($failed[$code] ?? 0) + 1;
        }
        curl_multi_remove_handle($mh, $done['handle']);
        $send();
    }
    clearstatcache();
}
file_put_contents("$base.finished", json_encode(['ok' => $ok, 'failed' => $failed]));
