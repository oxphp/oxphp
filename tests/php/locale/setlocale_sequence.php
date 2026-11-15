<?php

declare(strict_types=1);

// One fixed series of setlocale() calls, run both inside a request and by the
// PHP CLI with no ini file: test_setlocale_matches_php_cli compares the two.
// Returns, step by step, what the call returned and the LC_ALL name after it.
//
// The names are spelled for both libcs: a name one of them refuses is still a
// step, since both runs must refuse it alike. '@N' stands for what step N
// returned, so a step can take back an LC_ALL name in the spelling of the libc
// it runs on.
return static function (): array {
    // Read by an empty name, for its own category and for LC_ALL alike.
    putenv('LC_MONETARY=de_DE.UTF-8');

    $glibcAll = 'LC_CTYPE=C.UTF-8;LC_NUMERIC=de_DE.UTF-8;LC_TIME=C;LC_COLLATE=C;'
        . 'LC_MONETARY=C;LC_MESSAGES=C;LC_PAPER=C;LC_NAME=C;LC_ADDRESS=C;'
        . 'LC_TELEPHONE=C;LC_MEASUREMENT=C;LC_IDENTIFICATION=C';

    $steps = [
        [LC_NUMERIC, 'de_DE.UTF-8'],
        [LC_NUMERIC, 'de_DE.UTF-8'],
        [LC_CTYPE, 'C'],
        [LC_CTYPE, 'C.UTF-8'],
        [LC_TIME, 'POSIX'],
        [LC_COLLATE, 'C.UTF-8'],
        [LC_MONETARY, ''],
        [LC_ALL, '0'],
        [LC_ALL, 'C'],
        [LC_ALL, '@7'],
        [LC_ALL, 'de_DE.UTF-8'],
        [LC_CTYPE, 'C'],
        [LC_CTYPE, 'de_DE.UTF-8'],
        [LC_ALL, ''],
        [LC_COLLATE, str_repeat('x', 30)],
        [LC_ALL, str_repeat('y', 30) . ';C'],
        [LC_ALL, 'de_DE.UTF-8;C;C.UTF-8'],
        [LC_ALL, str_repeat('z', 23) . ';' . str_repeat('w', 24) . ';C'],
        [LC_ALL, 'C;C;C;C;;C'],
        [LC_CTYPE, 'a;b'],
        [LC_ALL, '0'],
        [LC_ALL, '@20'],
        [LC_NUMERIC, 'LC_NUMERIC=C;LC_TIME=C'],
        [LC_ALL, 'LC_CTYPE=C;LC_NUMERIC=C'],
        [LC_ALL, $glibcAll],
        [LC_ALL, '0'],
        [LC_ALL, 'C'],
        [LC_ALL, '@25'],
        [LC_ALL, 'no_SUCH.UTF-8'],
        [99, 'C'],
    ];

    $out = [];
    foreach ($steps as $i => [$category, $name]) {
        if (str_starts_with($name, '@')) {
            $name = $out[(int) substr($name, 1)]['ret'];
        }
        $ret = is_string($name) ? setlocale($category, $name) : null;
        $out[$i] = ['ret' => $ret, 'all' => setlocale(LC_ALL, '0')];
    }
    return $out;
};
