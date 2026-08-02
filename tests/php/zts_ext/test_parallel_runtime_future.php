<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('parallel_runtime_future', 'zts_ext');

// parallel bootstraps a PHP interpreter on a thread of its own, outside the
// worker pool. Nothing in the server arranges that, so what this checks is that
// it still works when the thread doing the bootstrapping is a worker serving a
// request rather than the main thread of a CLI process.
$runtime = new \parallel\Runtime();

$future = $runtime->run(static function (): array {
    return ['sapi' => PHP_SAPI, 'answer' => 6 * 7];
});

$t->assertNotNull('Runtime::run returned a Future', $future);

$value = $future->value();
$t->assertSame('the closure ran and its return value came back', $value['answer'] ?? null, 42);

// A parallel thread starts its own interpreter and does not inherit the request
// the calling worker is serving. Recording what it reports for PHP_SAPI pins
// which side of that boundary the thread lands on.
$t->assertNotEqual('the parallel thread reported a SAPI', $value['sapi'] ?? null, null);

// An anonymous channel is scoped to the objects holding it, unlike a named one —
// this is the path that behaves the same here as it does under a one-shot CLI
// run, and it is here so the named-channel test's result reads as a property of
// naming rather than of channels in general.
//
// The constructor is what makes an anonymous channel. Channel::make() takes a
// name first, so make(Channel::Infinite) does not do this: the constant is
// coerced to the string "-1" and the result is a channel named "-1", which on
// this server survives into the next request and collides there.
$channel = new \parallel\Channel(\parallel\Channel::Infinite);
$channel->send('anonymous-round-trip');
$t->assertSame('an anonymous channel round-trips a value', $channel->recv(), 'anonymous-round-trip');

$runtime->close();

$t->done();
