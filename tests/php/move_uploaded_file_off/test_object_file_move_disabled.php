<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// With move_uploaded_file in disable_functions, as in this profile, moveTo()
// reports a failed move instead of throwing "Invalid callback", and it never
// calls a function the application declares under that name. Sent via curl as
// -F "doc=@small.txt" -F "avatar=@/dev/null;filename=", the second field being
// an UPLOAD_ERR_NO_FILE entry.
$t = new TestCase('object_file_move_disabled', 'move_uploaded_file_off');

$t->assertFalse('move_uploaded_file() is not available in this profile', function_exists('move_uploaded_file'));

$req = oxphp_http_request();
$f = $req->file('doc');
$tmp = $f->tmpPath();
$dest = '/tmp/oxphp_uf_nomove_' . uniqid('', true) . '.txt';

$warnings = [];
$collect = static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
};
$moveCollecting = static function () use ($f, $dest, $collect): mixed {
    set_error_handler($collect, E_WARNING);
    try {
        return $f->moveTo($dest);
    } catch (Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    } finally {
        restore_error_handler();
    }
};

$t->assertSame('moveTo() reports the failed move', $moveCollecting(), false);
$t->assertCount('with one warning', $warnings, 1);
$t->assertMatch('that names the missing function', $warnings[0] ?? '', '/move_uploaded_file\(\) is not available/');
$t->assertFalse('nothing reaches the destination', is_file($dest));
$t->assertTrue('the upload stays where it was', is_uploaded_file($tmp));

// The TestCase error handler turns that warning into an exception, as frameworks do.
$t->assertThrows('a throwing error handler turns the warning into an exception', fn () => $f->moveTo($dest), ErrorException::class);

// An entry that is not a valid upload fails before the function is looked for,
// so there is no warning for the TestCase error handler to throw.
$t->assertSame('an invalid upload returns false without a warning', $req->file('avatar')?->moveTo($dest), false);

if (true) {
    function move_uploaded_file(string $from, string $to): bool
    {
        $GLOBALS['user_function_called'] = true;
        return true;
    }
}

$GLOBALS['user_function_called'] = false;
$warnings = [];
$t->assertTrue('the application has declared its own function', function_exists('move_uploaded_file'));
$t->assertSame('moveTo() reports the failed move all the same', $moveCollecting(), false);
$t->assertFalse('the function the application declared is not called', $GLOBALS['user_function_called']);
$t->assertCount('and the warning is raised as before', $warnings, 1);

$t->done();
