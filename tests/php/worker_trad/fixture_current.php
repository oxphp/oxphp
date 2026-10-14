<?php

declare(strict_types=1);

use OxPHP\Server\Worker;

// Reports whether the object Worker::current() returns belongs to this request.
//
// The objects below are the only ones this script creates, in this order:
// ?pad=N of them first, then $a and $b. An instance created by this request's
// first call is registered in this request's object store right after $b, so it
// takes the next handle, and it keeps that handle — and its class — while $c is
// created after it. An instance a previous request on the thread created lives
// in a store and a heap PHP has already torn down: reading its handle reads that
// freed memory, which goes on holding the handle the instance had there until
// PHP reuses it — and a live object of this request can hold that handle too.
// A caller sending a different pad on each request moves the handle this
// request's own instance would take, so an old instance cannot pass for a new
// one by having been created at the same point of the same script.
$pad = [];
$padCount = min(8, max(0, (int) ($_GET['pad'] ?? 0)));
for ($i = 0; $i < $padCount; $i++) {
    $pad[] = new stdClass();
}
$a = new stdClass();
$b = new stdClass();
$w = Worker::current();
$c = new stdClass();

$ids = array_map('spl_object_id', [...$pad, $a, $b, $w, $c]);

header('Content-Type: application/json');
echo json_encode([
    'worker' => oxphp_worker_id(),
    'ids' => $ids,
    'distinct' => count(array_unique($ids)) === count($ids),
    'created_here' => spl_object_id($w) === spl_object_id($b) + 1,
    'is_worker' => $w instanceof Worker,
    'same_within_request' => $w === Worker::current(),
]);
