<?php

declare(strict_types=1);

// Driven over HTTP by test_setlocale_survives_intl_on_other_worker; answers one
// JSON object.
//
//   ms=N       how long to keep calling NumberFormatter::parse()
//   marker=P   a file to create once the loop has started
//
// NumberFormatter::parse() switches libc's LC_NUMERIC to "C" and back around
// every call (numfmt_parse, ext/intl/formatter/formatter_parse.c, 8.4.27 and
// 8.5.12), on the process locale.
header('Content-Type: application/json');

$ms = max(1, min(10000, (int) ($_GET['ms'] ?? 1000)));
$formatter = new NumberFormatter('en_US', NumberFormatter::DECIMAL);
$parses = 0;
$wrong = 0;

$start = hrtime(true);
if (isset($_GET['marker'])) {
    file_put_contents((string) $_GET['marker'], '1');
}
do {
    if ($formatter->parse('1234.5') !== 1234.5) {
        $wrong++;
    }
    $parses++;
} while (hrtime(true) - $start < $ms * 1_000_000);
$end = hrtime(true);

echo json_encode([
    'worker' => oxphp_worker_id(),
    'start' => $start,
    'end' => $end,
    'parses' => $parses,
    'wrong' => $wrong,
]);
