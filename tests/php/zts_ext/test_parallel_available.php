<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('parallel_available', 'zts_ext');

// The premise every other test in this profile rests on. parallel does not build
// against a non-ZTS PHP at all, so a failure here means the image lost its ZTS
// base rather than that parallel misbehaved — worth separating, because that
// failure would otherwise show up as every test below failing at once.
$t->assertTrue('PHP is a ZTS build', PHP_ZTS);
$t->assertTrue('parallel is loaded', extension_loaded('parallel'));
$t->assertNotEqual('parallel reports a version', phpversion('parallel'), false);

// The classes are registered from the extension's MINIT, which on this server
// runs once for the process rather than once per request.
$t->assertTrue('parallel\Runtime exists', class_exists(\parallel\Runtime::class));
$t->assertTrue('parallel\Channel exists', class_exists(\parallel\Channel::class));
$t->assertTrue('parallel\Future exists', class_exists(\parallel\Future::class));

$t->done();
