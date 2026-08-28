<?php

// Worker entry for the abort-storm rig. One route, and a body big enough that
// a client which hangs up mid-request usually does so while the response is
// being built — which is the point of the rig: each of those hangups ends the
// request in a fatal, and it is fatals under an observer that the rig is about.

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
