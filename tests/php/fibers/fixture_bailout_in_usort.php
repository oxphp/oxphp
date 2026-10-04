<?php

declare(strict_types=1);

// Neighbour request for the tests that end a request inside usort() while another
// is in flight: ended by its time limit, with the data that leaves a lot behind
// unless ?rows= asks for fewer.

require_once __DIR__ . '/bailout_leak_probe.php';

OxphpBailoutLeak::sortUntilTimeout(OxphpBailoutLeak::rows((int) ($_GET['rows'] ?? OxphpBailoutLeak::BIG_ROWS)));

echo 'finished';
