<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The realistic shape of the same window: one triggered request among ordinary
// untriggered traffic.
//
// The neighbour here carries no trigger, so it runs in the ApmOnly mode this
// build gives every request that was not asked for. That is still a mode, and
// setting it on a slot shared with the worker thread both clears this request's
// collected spans and leaves the observer recording nothing for the rest of it.

$t = new TestCase('apm_neighbour_leaves_profile_alone', 'profilerfiber');

if (!function_exists('pf_third_fn')) {
    function pf_third_fn(int $n): int
    {
        return $n + 7;
    }
}

$t->assertTrue('this request is being profiled', OxPHP\Profile\is_active());

$sum = 0;
for ($i = 0; $i < 10; $i++) {
    $sum += pf_third_fn($i);
}
$t->assertSame('outer work ran before the suspend', $sum, 115);

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

fwrite($sock, "GET /tests/profilerfiber/fixture_inner_plain.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "Connection: close\r\n\r\n");

sleep(2);                                   // hooked: parks this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

$t->assertContains('the untriggered neighbour was served while this request was parked',
    $resp, 'INNER-PLAIN-OK');

$t->assertTrue('the observer is still recording after the resume',
    OxPHP\Profile\is_active());

for ($i = 0; $i < 10; $i++) {
    $sum += pf_third_fn($i);
}
$t->assertSame('outer work ran after the resume', $sum, 230);

$t->done();
