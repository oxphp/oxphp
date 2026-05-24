<?php
/**
 * Holding a strong ref to an in-transit Shared\* introduces a leak risk if
 * the value references the channel back. Channel send rejects reference
 * cycles with CycleException (parity with Shared\Map), so a self/cyclic
 * Shared\* that is never received cannot leak.
 */
header('Content-Type: text/plain');

$threw = function (callable $fn): bool {
    try {
        $fn();
        return false;
    } catch (\OxPHP\Shared\CycleException $e) {
        return true;
    }
};

// 1. Direct self-reference: $ch->send($ch).
$ch = new OxPHP\Shared\Channel(2);
if (!$threw(fn() => $ch->send($ch))) {
    echo "FAIL: direct self-reference not rejected\n";
    return;
}

// 2. Map -> Channel, then Channel -> Map closes the cycle on send.
$ch2 = new OxPHP\Shared\Channel(2);
$map = new OxPHP\Shared\Map();
$map->set('ch', $ch2);
if (!$threw(fn() => $ch2->send($map))) {
    echo "FAIL: map<->channel cycle not rejected\n";
    return;
}

// 3. Channel -> Channel cycle: a holds b in transit, then b->send(a).
$a = new OxPHP\Shared\Channel(2);
$b = new OxPHP\Shared\Channel(2);
$a->send($b);
if (!$threw(fn() => $b->send($a))) {
    echo "FAIL: channel<->channel cycle not rejected\n";
    return;
}

echo "OK\n";
