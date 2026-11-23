<?php

declare(strict_types=1);

// Inner self-request for the fibers/test_*error_mode* tests.
//
//   ?open=<path>  opens an oxehpark:// path and prints how the open ended;
//                 with &send=<query>, first sends that request and keeps its
//                 socket under the open's tag, so the request after a fatal
//                 open is one the worker takes with nothing resumed in between;
//   ?measure=<tag> prints what an error handler of its own hears of a notice
//                 and a warning — the state of the worker a request starts in —
//                 and, with &after=<tag>, whether that opener had fataled first.

require_once __DIR__ . '/error_mode_probe.php';

// The handlers a worker runs with are whatever the last request to set them
// installed — the outer test's, here, which throws on every error.
set_error_handler(null);
set_exception_handler(null);

header('Content-Type: text/plain');

if (isset($_GET['open'])) {
    echo 'OPENING:', $_GET['open'], "\n";
    if (isset($_GET['send'])) {
        $path = (string) $_GET['open'];
        OxphpErrorModeProbe::$sent[substr($path, strrpos($path, '/') + 1)] = OxphpErrorModeProbe::send((string) $_GET['send']);
    }
    $ended = OxphpErrorModeProbe::open((string) $_GET['open']);
    echo 'OPEN:', $ended, "\n";
} else {
    echo 'MEASURE:', json_encode(OxphpErrorModeProbe::measure((string) ($_GET['measure'] ?? 'm'))), "\n";
    if (isset($_GET['after'])) {
        echo 'PEER-FATALED:', (int) (OxphpErrorModeProbe::$fataling[(string) $_GET['after']] ?? false), "\n";
    }
}
