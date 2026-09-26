<?php

declare(strict_types=1);

// A request fiber that cannot be started is a failed request. The engine
// refuses to start a fiber whose stack would be smaller than its minimum, and
// fiber.stack_size is a directive any script may set. It is not one of those
// that travel with a request across its parks, so while this request sleeps
// its value stays on the worker, and the requests the event loop's tick starts
// in the meantime get no fiber at all. Three of them retire the worker.
//
// The same shape as test_breaker_eventloop_path: queue three requests, park so
// the tick takes them, and be the casualty of the retire they provoke —
// answered 503. The three ask for a file that only declares functions, so
// nothing but the failed start can make them count.
//
// No TestCase: this request does not get to report anything. What says the
// three failed for the reason above is the server log, asserted by the suite.

ini_set('fiber.stack_size', '1');

$socks = [];
for ($i = 1; $i <= 3; $i++) {
    $sock = @stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        continue;
    }
    fwrite($sock, "GET /tests/breaker/breaker_probe.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $socks[] = $sock;
}

oxphp_sleep(4.0);

foreach ($socks as $sock) {
    @fclose($sock);
}

echo "unreachable: three fibers that never started must retire this worker\n";
