<?php

declare(strict_types=1);

// The other side of the burst: a request that only uses the handle the worker
// already has, the way every request of an application built on `static $pdo`
// does. It constructs nothing, so it never reaches PDO's pooled lookup — all it
// contributes is a command on the shared connection, which is what a liveness
// check running unclaimed at the same moment lands in the middle of.
//
// prepare() and execute() rather than query(), because that is the pair the
// measurement on the load rig was built from: prepare() is the claimed entry
// point and execute() is the call that reported the error.
$started = microtime(true);
try {
    $pdo = $sharedState['ping_pdo'] ?? null;
    if (!$pdo instanceof PDO) {
        printf("ping-query-failed:no shared handle %.6f %.6f\n", $started, microtime(true));
        return;
    }

    $stmt = $pdo->prepare('SELECT CONNECTION_ID()');
    $stmt->execute();
    $id = $stmt->fetchColumn();

    printf("ping-query-done: id:%s %.6f %.6f\n", $id, $started, microtime(true));
} catch (\Throwable $e) {
    printf(
        "ping-query-failed:%s %.6f %.6f\n",
        str_replace("\n", ' ', $e->getMessage()),
        $started,
        microtime(true)
    );
}
