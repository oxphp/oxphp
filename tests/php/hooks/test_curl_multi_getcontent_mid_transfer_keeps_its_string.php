<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// curl_multi_getcontent() on a transfer still under way returns what has arrived
// so far, and that string stays as it was returned while the rest of the answer
// is written: ext/curl appends to the buffer it hands out, and a string sharing it
// would grow with the transfer, or be left pointing at memory the append freed.
// The answer comes in two parts (fixture_curl_two_parts.php); the string read
// between them is checked once the transfer is over.

$t = new TestCase('curl_multi_getcontent_mid_transfer_keeps_its_string', 'hooks');

$ch = curl_init('http://127.0.0.1:80/tests/hooks/fixture_curl_two_parts.php');
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$mh = curl_multi_init();
curl_multi_add_handle($mh, $ch);

$first = null;
$firstLength = 0;
$firstHash = '';
$runningAtFirst = 0;
$deadline = microtime(true) + 8.0;
do {
    curl_multi_exec($mh, $running);
    $content = curl_multi_getcontent($ch);
    if ($first === null && is_string($content) && $content !== '') {
        $first = $content;
        $firstLength = strlen($content);
        $firstHash = md5($content);
        $runningAtFirst = $running;
    }
    if ($running > 0) {
        curl_multi_select($mh, 1.0);
    }
} while ($running > 0 && microtime(true) < $deadline);
unset($content);
$final = curl_multi_getcontent($ch);
curl_multi_remove_handle($mh, $ch);
curl_multi_close($mh);

$t->meta('first_length', $firstLength);
$t->meta('first_now', is_string($first) ? strlen($first) : null);
$t->meta('final_length', is_string($final) ? strlen($final) : null);

$t->assertSame('the whole answer arrived', $final, str_repeat('a', 100) . str_repeat('b', 50));
$t->assertTrue('a part of it was read while the transfer was still running', $runningAtFirst >= 1 && $firstLength < 150);
$t->assertSame('the string read then still has the length it was returned with', is_string($first) ? strlen($first) : null, $firstLength);
$t->assertSame('and the same bytes', is_string($first) ? md5($first) : null, $firstHash);

$t->done();
