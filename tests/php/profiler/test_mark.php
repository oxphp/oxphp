<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

$t = new TestCase('mark', 'profiler');

// mark() is silent when no profile is active — never throws,
// returns void.
OxPHP\Profile\mark('outside-profile', ['origin' => 'pre-start']);
$t->assertTrue('mark before start completed without exception', true);

// mark() with a profile active — silent success (no PHP-visible
// side effects exposed from PHP).
OxPHP\Profile\start();
OxPHP\Profile\mark('inside-profile', ['origin' => 'post-start', 'user_id' => '42']);
$t->assertTrue('mark inside profile completed without exception', true);

// mark() with empty attrs.
OxPHP\Profile\mark('no-attrs');
$t->assertTrue('mark with default attrs completed without exception', true);

// mark() with mixed-type attrs (strings only — int values get
// stringified by the SDK).
OxPHP\Profile\mark('mixed', ['count' => '7', 'active' => '1']);
$t->assertTrue('mark with string attrs completed without exception', true);

OxPHP\Profile\stop();
$t->done();
