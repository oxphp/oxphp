<?php

declare(strict_types=1);

// Probe for fibers/test_curl_transfer_ended_by_a_retire_leaves_its_handle_usable:
// the worker was retired, and the request it killed inside curl_exec() was killed
// with the handle out of libcurl's transfer.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

$t = new TestCase('curl_transfer_ended_by_a_retire_leaves_its_handle_usable_probe', 'fibers');

OxphpBailoutLeak::assertRetired($t, 'curl_retire');

$state = OxphpBailoutLeak::state('curl_retire') ?? [];
$after = $state['after_the_kill'] ?? null;
$t->assertTrue('the killed request ran its shutdown function: ' . json_encode($state), is_array($after));
// PHP 8.6's ext/curl ends the transfer when the write callback throws (interface.c
// curl_write(), 8.6.0), so with ?throw=1 there is no wait left for the kill to end.
[$endedBy, $errno] = PHP_VERSION_ID >= 80600 && isset($_GET['throw'])
    ? ["the write callback's exception (CURLE_WRITE_ERROR)", CURLE_WRITE_ERROR]
    : ['the kill (CURLE_BAD_FUNCTION_ARGUMENT)', CURLE_BAD_FUNCTION_ARGUMENT];
$t->assertSame(
    "curl_exec() came back out of libcurl, its transfer ended by {$endedBy}",
    $after['errno'] ?? null,
    $errno
);
$t->assertSame('and libcurl let go of the handle: a multi handle takes it', $after['added'] ?? null, CURLM_OK);
$t->assertFalse('the code after curl_exec() did not run: the kill still ended the request', isset($state['returned']));
if (isset($_GET['throw'])) {
    $t->assertFalse(
        'the exception the write callback threw came back out of the killed wait: no callback ran after it',
        isset($state['callback_after_the_throw'])
    );
}

$t->done();
