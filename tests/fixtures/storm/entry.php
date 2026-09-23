<?php

// Worker entry for the abort-storm rig. One route, and a body big enough that
// a client which hangs up mid-request usually does so while the response is
// being built. Each of those hangups used to end the request in a fatal; the
// request now runs to its own end, and the rig reads whether the pool drains.

oxphp_worker(function () {
    header('Content-Type: application/json');

    $rows = [];
    for ($i = 0; $i < 200; $i++) {
        $rows[] = [
            'id'    => $i,
            'name'  => sprintf('row-%05d', $i),
            'score' => round($i * 1.234567, 4),
            'tags'  => ['alpha', 'beta', 'gamma'],
        ];
    }

    echo json_encode(['rows' => $rows], JSON_UNESCAPED_SLASHES);
});
