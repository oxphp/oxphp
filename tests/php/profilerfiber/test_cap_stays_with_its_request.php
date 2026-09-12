<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// PROFILER_MAX_SPANS is a per-request budget, and the counter it is measured
// against has to belong to the request too.
//
// This request burns past the cap (500 in this profile, see
// compose.profilerfiber.yml) and only then parks. The neighbour it parks across
// is profiled and tiny: on a counter shared by the worker thread it starts life
// already at the cap, records nothing at all, and — a tree with no spans being
// dropped rather than written — leaves no run behind, with the trigger honoured
// and not a line in the log to say so.
//
// The halves are read in two places. That the neighbour was served inside this
// window is read here; what each run ended up holding is read by
// profilerfiber/test_runs_are_separate, which runs last.

$t = new TestCase('cap_stays_with_its_request', 'profilerfiber');

if (!function_exists('pf_burn')) {
    function pf_burn(int $n): int
    {
        return $n & 1;
    }
}

$t->assertTrue('this request is being profiled', OxPHP\Profile\is_active());

$sum = 0;
for ($i = 0; $i < 2000; $i++) {
    $sum += pf_burn($i);
}
$t->assertSame('burn loop ran', $sum, 1000);

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

fwrite($sock, "GET /tests/profilerfiber/fixture_inner_small.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "X-OxPHP-Profile: test-token\r\n"
    . "Connection: close\r\n\r\n");

sleep(2);                                   // hooked: parks this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

$t->assertContains('the neighbour was served while this request was parked',
    $resp, 'INNER-SMALL-OK');

$t->done();
