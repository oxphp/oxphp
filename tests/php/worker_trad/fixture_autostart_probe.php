<?php

declare(strict_types=1);

use OxPHP\Server\Worker;

// Reports what the WorkerTradWakeProbe in this request's auto-started session
// got from Worker::current() when PHP woke it up — inside php_request_startup,
// before OxPHP's module had started the request — next to what the script gets.
//
// The session holds the probe and nothing else, so the probe is the first
// object this request creates and an instance created by its wake-up comes
// right after it. An instance a previous request on the thread got lives in a
// store and a heap PHP has already torn down.
$probe = $_SESSION['probe'] ?? null;
$woken = WorkerTradWakeProbe::$worker;
$w = Worker::current();
$c = new stdClass();

$ids = $probe instanceof WorkerTradWakeProbe ? [spl_object_id($probe), spl_object_id($w), spl_object_id($c)] : [];

header('Content-Type: application/json');
echo json_encode([
    'worker' => oxphp_worker_id(),
    'woken' => $probe instanceof WorkerTradWakeProbe && $woken !== null,
    'same' => $woken === $w,
    'created_here' => $ids !== [] && $ids[1] === $ids[0] + 1,
    'distinct' => $ids !== [] && count(array_unique($ids)) === count($ids),
    'is_worker' => $w instanceof Worker,
    'ids' => $ids,
]);
