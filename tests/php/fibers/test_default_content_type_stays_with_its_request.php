<?php

declare(strict_types=1);

// Whether the engine still owes a request its default Content-Type goes with
// the request when it parks. The engine keeps that per thread and clears it as
// it sends a request's headers or as the request sets a type of its own, so a
// request that found it left on the thread would resume to the answer of
// whichever request ran last.
//
// A holder that sets no type parks while a neighbour runs whole: the holder
// still gets the default type. A holder that sets its own type parks, and
// resumes while a neighbour that has sent nothing yet is parked too: each gets
// one Content-Type, its own.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$fixture = '/tests/fibers/fixture_default_content_type.php';
$types = static fn (array $response): array => array_values(array_filter(
    array_map('trim', explode("\r\n", strtolower($response['headers']))),
    static fn (string $line): bool => str_starts_with($line, 'content-type:')
));
$default = ['content-type: text/html; charset=utf-8'];

$sharedState['ct_holder'] = false;
$holderSock = thread_cell_send("$fixture?who=holder&park=600&edge=ct_holder");
$holderParked = thread_cell_wait_for($sharedState, 'ct_holder');
$neighbour = thread_cell_read(thread_cell_send("$fixture?who=neighbour&watch=ct_holder"));
$holder = thread_cell_read($holderSock);

$sharedState['ct_typed'] = false;
$sharedState['ct_untyped'] = false;
$typedSock = thread_cell_send("$fixture?who=typed&type=json&park=300&edge=ct_typed&watch=ct_untyped");
$typedParked = thread_cell_wait_for($sharedState, 'ct_typed');
$untypedSock = thread_cell_send("$fixture?who=untyped&park=900&edge=ct_untyped");
$untypedParked = thread_cell_wait_for($sharedState, 'ct_untyped');
$typed = thread_cell_read($typedSock);
$untyped = thread_cell_read($untypedSock);

$t = new TestCase('default_content_type_stays_with_its_request', 'fibers');
$t->meta('holder', $holder);
$t->meta('neighbour', $neighbour);
$t->meta('typed', $typed);
$t->meta('untyped', $untyped);

$t->assertTrue('the holder parked', $holderParked);
$t->assertSame(
    'the neighbour ran while the holder was parked',
    json_decode($neighbour['body'], true)['watched_parked'] ?? null,
    true
);
$t->assertSame('the neighbour got the default type', $types($neighbour), $default);
$t->assertSame('the holder got the default type too', $types($holder), $default);

$t->assertTrue('the typed request parked', $typedParked);
$t->assertTrue('the untyped request parked', $untypedParked);
$t->assertSame(
    'the typed request answered while the untyped one was parked',
    json_decode($typed['body'], true)['watched_parked'] ?? null,
    true
);
$t->assertSame('the typed request got its own type only', $types($typed), ['content-type: application/json']);
$t->assertSame('the untyped request got the default type', $types($untyped), $default);

$t->done();
