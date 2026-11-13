<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/bailout_leak_probe.php';

// Follows fibers/test_collection_while_releasing_frames_keeps_the_worker, on the
// worker it ran on.

$t = new TestCase('collection_while_releasing_frames_keeps_the_worker_probe', 'fibers');

$gc = OxphpBailoutLeak::state('collect_in_release')['gc'] ?? [];
$t->assertSame('the collection it asked for called both destructors of the cycle', $gc['cycle_destructed'] ?? null, 2);
$t->assertFalse('the guard was down when it asked', $gc['protected'] ?? null);
$t->assertGreaterThan('the collection freed the cycle', $gc['collected'] ?? 0, 1);
$t->assertFalse('and was over when it returned', $gc['running'] ?? null);
OxphpBailoutLeak::assertKept($t, 'collect_in_release');

$t->done();
