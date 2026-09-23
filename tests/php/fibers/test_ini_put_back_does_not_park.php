<?php

declare(strict_types=1);

// A request's ini changes are put back when it ends, and putting one back runs
// that directive's handler. A handler can release a value the request left, and
// with it run a destructor — so the put-back can run user code, after the
// request is over, while the server holds the reporting state it keeps quiet
// for the move. That code must not park: parked there, the request would leave
// the worker to its neighbours with that state still held, and come back to
// restore it over whatever they had done.
//
// assert.callback is such a directive. ini_set() enters it in the modified set;
// assert_options() then swaps the callback for a closure without touching that
// entry; and when this request, as the last one in flight, puts the directive
// back, its handler drops the closure, and with it the object below.
//
// The destructor sends the neighbour request and sleeps. The sleep is hooked in
// this profile, so if the destructor could park, the worker would take the
// neighbour while it slept; it cannot, so the neighbour runs only after this
// request has ended. The probe on the next suite line reads which.

require_once __DIR__ . '/ini_put_back_probe.php';

set_error_handler(null);
set_exception_handler(null);

OxphpIniPutBackProbe::$destructorRan = false;
OxphpIniPutBackProbe::$inDestructor = false;
OxphpIniPutBackProbe::$neighbourRan = false;
OxphpIniPutBackProbe::$neighbourSawDestructor = null;
OxphpIniPutBackProbe::$sock = null;

final class OxphpIniPutBackSleeper
{
    public function __destruct()
    {
        OxphpIniPutBackProbe::$destructorRan = true;
        OxphpIniPutBackProbe::$inDestructor = true;

        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock !== false) {
            fwrite($sock, "GET /tests/fibers/fixture_ini_put_back_neighbour.php HTTP/1.0\r\n"
                . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
            OxphpIniPutBackProbe::$sock = $sock;
        }

        usleep(500_000);

        OxphpIniPutBackProbe::$inDestructor = false;
    }
}

// Both calls are deprecated; the deprecations are not what this is about.
@ini_set('assert.callback', 'strlen');
$sleeper = new OxphpIniPutBackSleeper();
@assert_options(ASSERT_CALLBACK, static function () use ($sleeper): void {
});
unset($sleeper);

echo 'set';
