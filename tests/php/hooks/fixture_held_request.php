<?php

declare(strict_types=1);

// Inner request for hooks/test_request_object_refuses_another_request, served on
// the same worker thread while the outer request is parked. It calls every method
// of the Request object the outer request left in $sharedState and reports what
// each call did — the exception, or what it returned — then leaves its own
// object there, with its caches filled, for the outer request to try once this
// one has ended.
//
// Must not suspend: the outer request reads this one's state only after it has
// finished, and a suspension here would let the outer resume first.

use OxPHP\Http\Request;

$calls = require __DIR__ . '/request_object_calls.php';

$held = $sharedState['held_request'] ?? null;
$results = [];
if ($held instanceof Request) {
    foreach ($calls as $name => $call) {
        try {
            $results[$name] = 'no-throw: ' . json_encode($call($held));
        } catch (\Throwable $e) {
            $results[$name] = get_class($e) . ': ' . $e->getMessage();
        }
    }
}

$own = oxphp_http_request();
$own->query();
$own->attributes()->set('owner', 'inner');
$sharedState['inner_request'] = $own;

// The object answers in its own request's output callback, which runs as the
// request's output is flushed at its end.
ob_start(static fn(string $buffer): string
    => str_replace('"__ob_who__"', json_encode($own->payload('who')), $buffer));

header('Content-Type: application/json');
echo json_encode([
    'held'      => $held instanceof Request,
    'ob_who'    => '__ob_who__',
    'own_who'   => $own->payload('who'),
    'own_query' => $own->query('who'),
    'results'   => $results,
], JSON_UNESCAPED_SLASHES);
