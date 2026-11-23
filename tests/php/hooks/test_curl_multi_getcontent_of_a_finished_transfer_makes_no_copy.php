<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// curl_multi_getcontent() copies only what a transfer may still write to. Once the
// transfer has ended it returns ext/curl's own buffer, as PHP does without the
// hook, so a large answer read after its loop does not take twice the memory for
// as long as the handle lives. The end is seen two ways: ?after=loop reads once
// curl_multi_exec() has reported no transfer running and never calls
// curl_multi_info_read(); ?after=done reads on the CURLMSG_DONE message while
// another transfer in the handle is still under way.

$t = new TestCase('curl_multi_getcontent_of_a_finished_transfer_makes_no_copy', 'hooks');

$afterDone = ($_GET['after'] ?? '') === 'done';
$kb = 1024;
$base = 'http://127.0.0.1:80/tests/hooks/fixture_curl_body.php';

$mh = curl_multi_init();
$big = curl_init("{$base}?kb={$kb}");
curl_setopt_array($big, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
curl_multi_add_handle($mh, $big);
$slow = null;
if ($afterDone) {
    $slow = curl_init("{$base}?kb=1&delay=1500");
    curl_setopt_array($slow, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
    curl_multi_add_handle($mh, $slow);
}

$content = null;
$grew = null;
$hashAtRead = null;
$runningAtRead = null;
$read = static function () use ($big, &$content, &$grew, &$hashAtRead): void {
    $before = memory_get_usage();
    $content = curl_multi_getcontent($big);
    $grew = memory_get_usage() - $before;
    $hashAtRead = is_string($content) ? md5($content) : null;
};

$deadline = microtime(true) + 8.0;
do {
    curl_multi_exec($mh, $running);
    while ($afterDone && ($info = curl_multi_info_read($mh)) !== false) {
        if ($info['msg'] === CURLMSG_DONE && $info['handle'] === $big && $content === null) {
            $runningAtRead = $running;
            $read();
        }
    }
    if ($running > 0) {
        curl_multi_select($mh, 1.0);
    }
} while ($running > 0 && microtime(true) < $deadline);
if (!$afterDone) {
    $runningAtRead = $running;
    $read();
}
$slowBody = $slow !== null ? curl_multi_getcontent($slow) : null;

$t->meta('grew', $grew);
$t->meta('running_at_read', $runningAtRead);

$t->assertSame('the whole answer was read', is_string($content) ? strlen($content) : null, $kb * 1024);
if ($afterDone) {
    $t->assertTrue('it was read while another transfer in the handle was still under way', $runningAtRead >= 1);
    $t->assertSame('which then completed', $slowBody, str_repeat('x', 1024));
} else {
    $t->assertSame('it was read after the loop saw no transfer running', $runningAtRead, 0);
}
$t->assertTrue('reading it allocated no copy of it', is_int($grew) && $grew < 64 * 1024);
$t->assertSame('and it did not change while the loop went on', is_string($content) ? md5($content) : null, $hashAtRead);

curl_multi_remove_handle($mh, $big);
if ($slow !== null) {
    curl_multi_remove_handle($mh, $slow);
}
curl_multi_close($mh);

$t->done();
