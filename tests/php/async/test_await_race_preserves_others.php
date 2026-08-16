<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_race_preserves_others', 'async');

// Same restoration contract as await_any (success path). Three promises
// that all fulfill; whichever one wins, the other two must remain
// awaitable individually with their own values. Worker-pool scheduling
// can pick any of them as winner — the test does not assume a specific
// outcome, only that store_promise(id, rx) paired correctly for the two
// losers.
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

$winner = oxphp_async_await_race($promises, 5.0);

$t->assertContains('winner id is one of the inputs', json_encode($promises), (string) $winner['id']);
$t->assertSame(
    'winner value matches its own tag',
    $winner['value'],
    $id_to_tag[$winner['id']]
);

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
