<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_persistent_connect_holds_thread', 'hooks');

// A persistent connection is entered in the persistent list under its key before it
// is connected, and until the connect returns it has no descriptor. A second fiber
// asking for the same key finds it there, takes "no descriptor" for a dead
// connection and closes it — under the fiber that is still connecting. So a
// persistent connect is the one connect the net category does not park: it keeps
// the thread for its wait, and nothing can ask for the key in the meantime.
//
// The control is test_net_connect_task_multiplex, where the same two connects to an
// address that answers nothing, not persistent, overlap on the one async worker.
// Here they must run one after the other, and both must come back as the ordinary
// failed connect — not a crash, which is what the closed stream would be.
$silent = net_silent_address(2);

$tasks = [];
for ($i = 0; $i < 2; $i++) {
    $tasks[] = oxphp_async(function () use ($silent): array {
        $started = microtime(true);
        $sock = @pfsockopen($silent, 80, $errno, $errstr, 1.0);
        $finished = microtime(true);
        if (is_resource($sock)) {
            fclose($sock);
        }
        return ['connected' => $sock !== false, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

foreach ($results as $i => $r) {
    $t->assertFalse("task {$i}: nothing answers at that address, so the connect failed the ordinary way", $r['connected']);
}

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));
$t->assertGreaterThan(
    'the two persistent connects ran one after the other (serial is about 2s, parked would be about 1s)',
    $span,
    1.8
);

$t->done();
