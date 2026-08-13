<?php
// A floor that belongs to php.ini, and a bootstrap that is not allowed to move it.
//
// phar.readonly is not an ordinary boolean. Its handler remembers the value it
// is given at the startup stage as the floor and from then on refuses every
// change that would relax it — that is how PHP keeps a php.ini that forbids
// writing phars from being talked out of it at runtime. An environment that
// does need to write them puts `phar.readonly = 0` in php.ini, which sets the
// floor to 0 and leaves the directive movable in both directions afterwards.
//
// On top of that an application may tighten the directive to 1 at bootstrap and
// relax it around the one place that builds a phar. Under every other SAPI that
// works, because the bootstrap's ini_set() is a runtime change like any other
// and the floor stays where php.ini put it.
//
// A worker has one request startup for its whole life, and its bootstrap values
// have to be carried past the request heap they were allocated in. What that
// carrying must not do is re-announce them at the startup stage, because for
// this directive the startup stage does not mean "refresh a cached value", it
// means "this is the floor". Announced there, the bootstrap's 1 becomes the
// floor and no request can ever open a phar for writing again — silently, since
// ini_set() only returns false and nothing is logged.
//
// Listed twice in the suite, and the repetition is load-bearing: the first run
// proves a request can still relax the directive, and the second proves the
// first one's change ended with it, because the entry assertion below is the
// bootstrap value again.
//
// Written without test_helper.php: the tests under tests/php/worker pull it in
// with a bare `require`, so a worker serving two of them fatals on the class
// redeclare.

// From the worker boot scope (tests/fixtures/worker/worker_entry.php), read
// there before the bootstrap tightened the directive.
if (!isset($pharReadonlyPhpIni)) {
    http_response_code(500);
    echo "FAIL: \$pharReadonlyPhpIni is not in scope — this test needs the worker"
        . " entry file at tests/fixtures/worker/worker_entry.php\n";
    return;
}

$fail = [];

if ($pharReadonlyPhpIni !== '0') {
    // Setup, not a defect: with the floor already at 1 no build can relax the
    // directive, so the assertions below would fail on a correct one too.
    $fail[] = 'php.ini has phar.readonly = ' . var_export($pharReadonlyPhpIni, true)
        . ', so the floor is already 1 and there is nothing to measure — this'
        . ' profile needs tests/hooks.ini mounted into conf.d';
} else {
    $onEntry = ini_get('phar.readonly');
    if ($onEntry !== '1') {
        $fail[] = 'on entry phar.readonly = ' . var_export($onEntry, true)
            . ", expected '1', the value the worker's bootstrap set — either the"
            . ' bootstrap did not run or a previous request left this changed';
    }

    $previous = ini_set('phar.readonly', '0');
    if ($previous === false) {
        $fail[] = 'ini_set(\'phar.readonly\', \'0\') was refused while php.ini'
            . " allows it — the floor moved to the bootstrap's value, so no"
            . ' request on this worker can write a phar any more';
    } elseif (($seen = ini_get('phar.readonly')) !== '0') {
        $fail[] = 'ini_set() reported success but phar.readonly = '
            . var_export($seen, true);
    }
}

if ($fail !== []) {
    http_response_code(500);
    echo 'FAIL: ' . implode('; ', $fail) . "\n";
    return;
}

echo "OK\n";
