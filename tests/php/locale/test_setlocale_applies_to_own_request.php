<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// setlocale() within one request: what it returns for a change, a query, an
// LC_ALL name with categories that differ, several candidates and a name PHP
// refuses — and that the locale it sets is the one escapeshellarg() then uses.
$t = new TestCase('test_setlocale_applies_to_own_request', 'locale');

$base = setlocale(LC_CTYPE, '0');
$baseAll = setlocale(LC_ALL, '0');
$baseEscaped = bin2hex(escapeshellarg("a\xffb"));
$t->assertNotEqual('the server starts a request on an LC_CTYPE other than C', $base, 'C');

$t->assertSame('switching LC_CTYPE to C returns C', setlocale(LC_CTYPE, 'C'), 'C');
$t->assertSame('a query returns the new name', setlocale(LC_CTYPE, '0'), 'C');
$t->assertNotEqual('escapeshellarg() follows the change', bin2hex(escapeshellarg("a\xffb")), $baseEscaped);
$t->assertSame('changing another category returns its name', setlocale(LC_NUMERIC, 'C'), 'C');
$t->assertSame('and keeps the LC_CTYPE set before', setlocale(LC_CTYPE, '0'), 'C');

$mixed = setlocale(LC_ALL, '0');
$t->assertNotEqual('LC_ALL changes with LC_CTYPE', $mixed, $baseAll);
$t->assertSame('LC_ALL takes back the name it returned at the start', setlocale(LC_ALL, $baseAll), $baseAll);
$t->assertSame('which brings LC_CTYPE back', setlocale(LC_CTYPE, '0'), $base);
$t->assertSame('and escapeshellarg() with it', bin2hex(escapeshellarg("a\xffb")), $baseEscaped);
$t->assertSame('LC_ALL takes back the later name too', setlocale(LC_ALL, $mixed), $mixed);
$t->assertSame('which sets LC_CTYPE to C again', setlocale(LC_CTYPE, '0'), 'C');

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
});
$tooLong = str_repeat('x', 255);
$refused = setlocale(LC_CTYPE, $tooLong);
$fromArray = setlocale(LC_CTYPE, [$tooLong, $base]);
$fromArgs = setlocale(LC_CTYPE, $tooLong, 'C');
restore_error_handler();

$t->assertFalse('a name PHP refuses returns false', $refused);
$t->assertSame('each refused name warns', $warnings, array_fill(0, 3, 'setlocale(): Specified locale name is too long'));
$t->assertSame('the first candidate that works wins, from an array', $fromArray, $base);
$t->assertSame('the first candidate that works wins, from the arguments', $fromArgs, 'C');

$fromEnv = setlocale(LC_ALL, '');
$t->assertType('an empty name takes the locale from the environment', $fromEnv, 'string');
$t->assertSame('which a query then returns', setlocale(LC_ALL, '0'), $fromEnv);

if (PHP_VERSION_ID >= 80500) {
    $t->assertThrows(
        'under strict_types an int is not a locale name',
        static fn () => setlocale(LC_CTYPE, 0),
        \TypeError::class
    );
} else {
    $t->assertSame('an int 0 is a query', setlocale(LC_CTYPE, 0), setlocale(LC_CTYPE, '0'));
}

$t->done();
