<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// fiber.stack_size is not one of the directives that travel with a request
// across its parks: while this request sleeps, the value it set stays on the
// worker. The fibers the event loop's tick starts in the meantime for other
// requests are the worker's, not this request's — they stay in its pool and
// serve every request after it — so they are sized as the worker serves, not
// by what this request set.
//
// 32K is enough for a fiber to start and less than the zend.reserved_stack_size
// the engine keeps free below its call-stack limit, so a fiber sized by it
// leaves the handler no stack at all and a request on it cannot run: on PHP 8.4
// and 8.5 it throws "Maximum call stack size of 0 bytes" on its first call and
// answers 500.
//
// Right after a replacement worker has booted, so the pool holds only the fiber
// this request runs on and each of the three gets a fiber started for it.
//
// The value is still this request's for a Fiber it starts itself, once the
// worker has started its own in between: such a fiber gets 32K and fails its
// first call, where one given the worker's size would run it.

$t = new TestCase('breaker_parked_stack_size', 'breaker');

ini_set('fiber.stack_size', '32K');

$socks = [];
for ($i = 1; $i <= 3; $i++) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("probe {$i} connected ({$errstr})", $sock !== false);
    if ($sock === false) {
        continue;
    }
    fwrite($sock, "GET /tests/breaker/stack_size_probe.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $socks[$i] = $sock;
}

// Parked, so the tick takes the three while this request's value stands.
oxphp_sleep(2.0);

foreach ($socks as $i => $sock) {
    stream_set_timeout($sock, 5);
    $response = (string) stream_get_contents($sock);
    fclose($sock);
    $t->assertContains("probe {$i} answered 200", explode("\r\n", $response, 2)[0], ' 200 ');
    $t->assertContains("probe {$i} ran its handler", $response, 'stack-size-probe-ok');
}

try {
    $own = (new \Fiber(static fn (): string => 'ran'))->start();
} catch (\Error $e) {
    $own = $e->getMessage();
}
$t->assertContains("a Fiber of the request's own is sized by its setting", (string) $own, 'Maximum call stack size of 0 bytes');

$t->done();
