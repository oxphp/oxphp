<?php

declare(strict_types=1);

// A request ended inside a cURL transfer — by a fatal or its time limit, raised
// in one of the transfer's callbacks — retires the worker once nothing else is
// in flight on it, and the probe on the next line reads that it did.
//
// The bailout leaves through libcurl in the middle of the transfer, past the
// code that records the callback's end: ext/curl keeps the handle marked as
// inside a callback, and libcurl refuses every later transfer on it as a
// recursive call. A handle the worker keeps for every request, as an
// application keeps its HTTP client, then fails every transfer after it, and
// nothing on the PHP side can clear either mark.
//
// ?via=multi runs the transfer through curl_multi_exec(); ?via=pause pauses it
// from the callback and resumes it with curl_pause(), inside which libcurl hands
// the held-back data to the callback again; ?via=remove and ?via=close leave it
// paused and end it with curl_multi_remove_handle() / curl_multi_close(), inside
// which libcurl reports the unfinished transfer's progress one last time;
// ?via=timeout ends it by the request's time limit instead of a fatal;
// ?via=after lets the transfer return and fails the request after it, which
// leaves nothing to retire for.
// The peer is the server's internal listener, which PHP does not serve: this
// profile has a single PHP worker, and it is busy running this request.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';
require_once __DIR__ . '/destruct_probe.php';

$via = $_GET['via'] ?? 'easy';
OxphpBailoutLeak::arm('curl_' . $via);

$h = curl_init('http://127.0.0.1:9090/metrics');
$sharedState['curl_' . $via] = $h;
$paused = false;
$inPause = false;
$inRemove = false;
$endsPaused = $via === 'remove' || $via === 'close';
if ($endsPaused) {
    curl_setopt($h, CURLOPT_NOPROGRESS, false);
    curl_setopt($h, CURLOPT_XFERINFOFUNCTION, static function () use ($via, &$inRemove): int {
        if ($inRemove) {
            OxphpBailoutLeak::note('curl_' . $via, 'failed_inside_the_removal', true);
            oxphp_destruct_probe_fatal();
        }

        return 0;
    });
}
curl_setopt($h, CURLOPT_WRITEFUNCTION, static function ($h, string $data) use ($via, $endsPaused, &$paused, &$inPause): int {
    if ($endsPaused) {
        $paused = true;

        return CURL_WRITEFUNC_PAUSE;
    }
    if ($via === 'pause') {
        if (!$paused) {
            $paused = true;

            return CURL_WRITEFUNC_PAUSE;
        }
        OxphpBailoutLeak::note('curl_pause', 'failed_inside_curl_pause', $inPause);
    }
    if ($via === 'timeout') {
        set_time_limit(1);
        $spin = 0;
        while (true) {
            $spin++;
        }
    }
    if ($via !== 'after') {
        oxphp_destruct_probe_fatal();
    }

    return strlen($data);
});

if ($via === 'multi' || $via === 'pause' || $endsPaused) {
    $mh = curl_multi_init();
    curl_multi_add_handle($mh, $h);
    do {
        $status = curl_multi_exec($mh, $running);
        if ($paused && $endsPaused) {
            $inRemove = true;
            if ($via === 'remove') {
                curl_multi_remove_handle($mh, $h);
            } else {
                curl_multi_close($mh);
            }
            $inRemove = false;
            break;
        }
        if ($paused && !$inPause) {
            $inPause = true;
            curl_pause($h, CURLPAUSE_CONT);
            $inPause = false;
        }
        if ($running) {
            curl_multi_select($mh, 1.0);
        }
    } while ($running && $status === CURLM_OK);
} else {
    curl_exec($h);
}

if ($via === 'after') {
    oxphp_destruct_probe_fatal();
}

echo 'not reached';
