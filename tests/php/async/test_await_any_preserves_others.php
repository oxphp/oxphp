<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_preserves_others', 'async');

// Three promises that all fulfill, with different delays. After await_any
// returns the winner, the other two (whichever they are — worker-pool
// scheduling can pick any of them as the first to settle) must remain
// awaitable individually with their own values.
//
// This guards against parallel-vec desync in the dispatcher's victory
// path: if id_vec.swap_remove(idx) gets out of step with select_all's
// internal swap_remove, store_promise(id, rx, ...) pairs the wrong id
// with the wrong receiver and the loser-await would either hang, return
// the wrong value, or throw "unknown promise id".
//
// Each closure returns its own input, so the result tells us which
// promise produced it. We then verify that for every non-winner id,
// awaiting it returns ITS OWN tag (not the winner's, not someone else's).
$tags = ['A', 'B', 'C'];
$promises = [];
$id_to_tag = [];
foreach ($tags as $i => $tag) {
    $delay = ($i + 1) * 30_000; // 30ms, 60ms, 90ms
    $pid = oxphp_async(function (string $tag, int $delay): string {
        usleep($delay);
        return $tag;
    }, $tag, $delay);
    $promises[] = $pid;
    $id_to_tag[$pid] = $tag;
}

$winner = oxphp_async_await_any($promises, 5.0);

// Whichever id won, its value must equal its tag.
$t->assertContains('winner id is one of the inputs', json_encode($promises), (string) $winner['id']);
$t->assertSame(
    'winner value matches its own tag',
    $winner['value'],
    $id_to_tag[$winner['id']]
);

// The two non-winner ids must each still be awaitable and produce their
// own tag — not winner's, not each other's.
$non_winners = array_values(array_filter($promises, fn($pid) => $pid !== $winner['id']));
$t->assertCount('two non-winners remain', $non_winners, 2);

foreach ($non_winners as $pid) {
    $value = oxphp_async_await($pid, 5.0);
    $t->assertSame(
        "non-winner {$pid} preserved with its own tag",
        $value,
        $id_to_tag[$pid]
    );
}

$t->done();
