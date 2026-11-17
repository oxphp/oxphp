<?php

declare(strict_types=1);

// Inner request for fibers/test_usort_comparator_stays_with_its_request.
// ?who= names it, ?park=N is how long its comparator parks on its first call.
// Every call of the comparator logs whose comparator it is and which request
// was running when it was called; each request marks itself running as it
// starts and again whenever its park returns. Both edges of the park are
// published in $sharedState.

$who = (string) ($_GET['who'] ?? '-');
$park = (int) ($_GET['park'] ?? 0);
$parkedKey = 'cell_usort_parked_' . $who;
$sharedState['cell_usort_running'] = $who;

$first = true;
$otherParked = [];
$values = [5, 3, 8, 1, 9, 2, 7, 4, 6];
$error = null;
try {
    usort($values, static function (int $a, int $b) use ($who, $park, $parkedKey, &$first, &$otherParked, &$sharedState): int {
        $sharedState['cell_usort_log'][] = ['comparator' => $who, 'running' => $sharedState['cell_usort_running']];
        if ($first) {
            $first = false;
            $sharedState[$parkedKey] = true;
            oxphp_usleep($park * 1000);
            $sharedState[$parkedKey] = false;
            $sharedState['cell_usort_running'] = $who;
            foreach ($sharedState as $key => $value) {
                if (is_string($key) && str_starts_with($key, 'cell_usort_parked_') && $key !== $parkedKey) {
                    $otherParked[$key] = $value;
                }
            }
        }

        return $a <=> $b;
    });
} catch (\Throwable $e) {
    $error = get_class($e) . ': ' . $e->getMessage();
}

echo json_encode(['who' => $who, 'sorted' => $values, 'error' => $error, 'others_parked_at_resume' => $otherParked]);
