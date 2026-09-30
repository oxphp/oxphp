<?php

// APM Redis auto-instrumentation e2e fixture.
//
// The APM plugin registers the phpredis methods whose declared names are
// mixed-case (hGet, hSet, lPush, rPush) in lowercase. The hook wrapper has to
// find the original handler for a call regardless of that difference, or the
// call returns NULL without ever reaching Redis. Each write below is checked by
// reading it back, through three call forms that reach the wrapper:
//   - the declared spelling            ($r->rPush)
//   - another spelling at the call site ($r->rpush)
//   - a first-class callable           ($r->lPush(...)) — the engine calls a
//     copy of the function held by the closure, not the class's own entry
//
// Assertions live in tests/apm_redis_spans.sh.

$r = new Redis();
$r->connect(getenv('DB_REDIS_HOST') ?: 'redis', 6379);
$r->del('k1', 'k2', 'h');

$push = $r->lPush(...);

$out = [
    'rPush' => $r->rPush('k1', 'a'),
    'rpush' => $r->rpush('k2', 'b'),
    'lPush' => $push('k1', 'c'),
    'hSet'  => $r->hSet('h', 'f', 'v'),
    'hGet'  => $r->hGet('h', 'f'),
    'len1'  => $r->lLen('k1'),
    'len2'  => $r->lLen('k2'),
];

echo json_encode($out);
