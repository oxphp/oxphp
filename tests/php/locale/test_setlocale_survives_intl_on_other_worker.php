<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// This request keeps changing its locale while a request on the other worker
// calls NumberFormatter::parse() in a loop, which sets libc's LC_NUMERIC to "C"
// and back on the process locale, from that thread. Every name this request
// reads back is one it set: intl's "C" never becomes its LC_NUMERIC.
$t = new TestCase('test_setlocale_survives_intl_on_other_worker', 'locale_intl');

if (!extension_loaded('intl')) {
    $t->assertTrue('intl is loaded', false);
    $t->done();
    exit;
}

// Two names, neither of them intl's "C", and on glibc both with a decimal comma.
// musl's localeconv() answers "." under any locale (src/locale/localeconv.c,
// 1.2.6), so there only the names can go wrong.
$numeric = ['de_DE.UTF-8', 'fr_FR.UTF-8'];
$t->assertSame('this request sets its LC_NUMERIC', setlocale(LC_NUMERIC, $numeric[0]), $numeric[0]);
$expect = $numeric[0];
$formatted = sprintf('%.1f', 1.5);

$marker = sys_get_temp_dir() . '/oxphp-locale-intl-' . bin2hex(random_bytes(6));
$mh = curl_multi_init();
$ch = curl_init('http://127.0.0.1/tests/locale/fixture_locale_intl.php?ms=3000&marker=' . rawurlencode($marker));
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 20]);
curl_multi_add_handle($mh, $ch);

$deadline = hrtime(true) + 5_000_000_000;
do {
    curl_multi_exec($mh, $running);
    usleep(1000);
} while (!is_file($marker) && $running && hrtime(true) < $deadline);
$t->assertTrue('the other request started parsing', is_file($marker));

$monetary = ['C', $numeric[1]];
$changes = 0;
$reads = 0;
$foreign = [];
$misformatted = 0;
$start = hrtime(true);
while (hrtime(true) - $start < 2_000_000_000) {
    $step = $reads++;
    if ($step % 16 === 15) {
        $all = setlocale(LC_ALL, '0');
        setlocale(LC_ALL, 'C');
        setlocale(LC_ALL, (string) $all);
        $changes += 2;
    } elseif ($step % 4 === 3) {
        $next = $numeric[$expect === $numeric[0] ? 1 : 0];
        if (setlocale(LC_NUMERIC, $next) === $next) {
            $expect = $next;
        }
        $changes++;
    } else {
        setlocale(LC_MONETARY, $monetary[$step & 1]);
        $changes++;
    }

    $now = setlocale(LC_NUMERIC, '0');
    if ($now !== $expect) {
        $foreign[(string) $now] = ($foreign[(string) $now] ?? 0) + 1;
        setlocale(LC_NUMERIC, $expect);
    }
    if (sprintf('%.1f', 1.5) !== $formatted) {
        $misformatted++;
    }
}
$end = hrtime(true);

do {
    $status = curl_multi_exec($mh, $running);
    if ($running) {
        curl_multi_select($mh, 1.0);
    }
} while ($running && $status === CURLM_OK);
$other = json_decode((string) curl_multi_getcontent($ch), true);
curl_multi_remove_handle($mh, $ch);
curl_multi_close($mh);
@unlink($marker);

// The premise: both loops ran, on different threads, the whole time this one did.
$t->assertTrue('the other request answered', is_array($other));
$other = is_array($other) ? $other : [];
$t->assertTrue(
    'it ran on the other worker',
    is_int($other['worker'] ?? null) && $other['worker'] !== oxphp_worker_id()
);
$t->assertTrue(
    'and was parsing from before this loop started until after it ended',
    ($other['start'] ?? PHP_INT_MAX) <= $start && ($other['end'] ?? 0) >= $end
);
$t->assertGreaterThan('it parsed many times', $other['parses'] ?? 0, 10_000);
$t->assertSame('and every parse came out right', $other['wrong'] ?? null, 0);
$t->assertGreaterThan('this request changed its locale many times', $changes, 10_000);
$t->meta('parses', $other['parses'] ?? null);
$t->meta('changes', $changes);

$t->assertSame('every LC_NUMERIC read back is one this request set', $foreign, []);
$t->assertSame('and formatting followed it every time', $misformatted, 0);
$t->assertSame('the request ends on the LC_NUMERIC it set last', setlocale(LC_NUMERIC, '0'), $expect);

$t->done();
