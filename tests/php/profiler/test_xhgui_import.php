<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

$t = new TestCase('xhgui_import', 'xhgui');

// 1) Trigger a profile in a *separate* request via a self-hit. Profile
//    state (span counters, xhgui push) is committed by the server only
//    after the request completes, so using a sibling request lets us
//    then observe its effects in /__profiler/stats and xhgui.
$ch = curl_init('http://127.0.0.1/tests/profiler/test_is_active.php');
curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
curl_setopt($ch, CURLOPT_HTTPHEADER, ['X-OxPHP-Profile: test-token']);
curl_setopt($ch, CURLOPT_TIMEOUT, 5);
curl_exec($ch);
$trigger_code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
curl_close($ch);
$t->assertSame('self-trigger 200', $trigger_code, 200);

// 2) Also exercise the imperative SDK from this request — covers the
//    SDK path even though its effects won't be observable until after
//    this test returns.
OxPHP\Profile\start();
$fn = function (): string { return strtoupper('ox_xhgui_e2e'); };
$fn();
OxPHP\Profile\stop();

// 3) Let the async HTTP push + disk write from the self-hit land.
usleep(500 * 1000);

// 4) Poll xhgui's run-list page (served at "/") for up to ~5 s.
$found = false;
$last_code = 0;
$last_body = '';
for ($i = 0; $i < 10; $i++) {
    $ch = curl_init('http://xhgui/?sort=wt');
    curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
    curl_setopt($ch, CURLOPT_TIMEOUT, 2);
    $body = curl_exec($ch);
    $last_code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
    curl_close($ch);
    $last_body = (string) $body;
    if ($last_code === 200 && strlen($last_body) > 20) {
        $found = true;
        break;
    }
    usleep(500 * 1000);
}
$t->assertTrue("xhgui root returned data within 5 s (last code=$last_code)", $found);

// 5) Sanity: /__profiler/stats reports the run was captured (our side).
$ch2 = curl_init('http://127.0.0.1:9090/__profiler/stats');
curl_setopt($ch2, CURLOPT_HTTPHEADER, ['Authorization: Bearer test-token']);
curl_setopt($ch2, CURLOPT_RETURNTRANSFER, true);
$stats_body = curl_exec($ch2);
$stats_code = (int) curl_getinfo($ch2, CURLINFO_HTTP_CODE);
curl_close($ch2);
$t->assertSame('profiler stats 200', $stats_code, 200);
$stats = json_decode((string) $stats_body, true);
$t->assertTrue('profiler captured at least one run',
    is_array($stats) && ($stats['spans_collected_total'] ?? 0) >= 1);

// 6) Push failures should still be zero (xhgui ACKed the envelope).
$t->assertTrue('no http push failures',
    is_array($stats) && (int) ($stats['http_push_failures_total'] ?? -1) === 0);

$t->done();
