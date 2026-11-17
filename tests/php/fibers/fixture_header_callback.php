<?php

declare(strict_types=1);

// Inner request for fibers/test_header_callback_stays_with_its_request. ?who=
// names it. ?cb=1 registers a header callback that adds X-Header-Callback with
// that name; ?cb=nested registers one that registers that callback while the
// headers are being sent, which the engine accepts and keeps after they have
// gone out, holding the last reference to an object that counts its release
// in a static the worker keeps across requests. ?park=N then parks it for N
// ms, publishing both edges of the park in $sharedState. ?busy=N keeps it busy
// for N ms instead, without parking, and publishes that it is done. Every
// request reports whether the parking one was parked while it ran, whether the
// busy one was done, and whether the test had resumed by then.

if (!class_exists('ThreadCellCallbackProbe', false)) {
    final class ThreadCellCallbackProbe
    {
        public static int $released = 0;

        public function __destruct()
        {
            ++self::$released;
        }
    }
}

$who = (string) ($_GET['who'] ?? '-');

$addHeader = static function () use ($who): void {
    header("X-Header-Callback: $who");
};
if (($_GET['cb'] ?? '') === 'nested') {
    $probe = new ThreadCellCallbackProbe();
    header_register_callback(static function () use ($probe, $who): void {
        header_register_callback(static function () use ($probe, $who): void {
            header("X-Header-Callback: $who");
        });
    });
    unset($probe);
} elseif (isset($_GET['cb'])) {
    header_register_callback($addHeader);
}

// Busy rather than parked: a request sent meanwhile is then queued, and the
// worker starts it as this one ends.
if (isset($_GET['busy'])) {
    $until = microtime(true) + (int) $_GET['busy'] / 1000;
    while (microtime(true) < $until) {
    }
    $sharedState['cell_cb_busy_done'] = true;
}

if (isset($_GET['park'])) {
    $sharedState['cell_cb_parked'] = true;
    oxphp_usleep((int) $_GET['park'] * 1000);
    $sharedState['cell_cb_parked'] = false;
}

echo json_encode([
    'who' => $who,
    'holder_parked' => $sharedState['cell_cb_parked'] ?? null,
    'reader_resumed' => $sharedState['cell_cb_reader_resumed'] ?? null,
    'busy_done' => $sharedState['cell_cb_busy_done'] ?? null,
]);
