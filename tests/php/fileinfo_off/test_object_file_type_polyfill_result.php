<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A mime_content_type() the application declares where the built-in is missing
// can return anything, and type() owns what it returns. Sent via curl as
// -F "doc=@small.txt".
$t = new TestCase('object_file_type_polyfill_result', 'fileinfo_off');

$t->assertFalse('the built-in mime_content_type() is not available in this profile', function_exists('mime_content_type'));

final class PolyfillResult
{
    public function __destruct()
    {
        $GLOBALS['released'] = true;
    }
}

final class ThrowsOnDestruct
{
    public function __destruct()
    {
        throw new RuntimeException('destructor');
    }
}

if (!function_exists('mime_content_type')) {
    function mime_content_type(string $path): mixed
    {
        // A local of this frame: engine leaves the frame type() called with the
        // return value set, and only then destroys this.
        $guard = $GLOBALS['guard'] ? new ThrowsOnDestruct() : null;
        return ($GLOBALS['answer'])();
    }
}

$GLOBALS['guard'] = false;
$req = oxphp_http_request();

// The result is released as soon as type() is done with it, not at request end.
$GLOBALS['released'] = false;
$GLOBALS['answer'] = static fn () => new PolyfillResult();
$first = $req->file('doc');
$t->assertSame('a result that is not a string falls back', $first->type(), 'application/octet-stream');
$t->assertTrue('the result is destroyed before type() returns', $GLOBALS['released']);

// Releasing the result can throw; that call detected nothing, so it keeps nothing.
$GLOBALS['answer'] = static fn () => new ThrowsOnDestruct();
$throwing = $req->file('doc');
$t->assertThrows('a destructor of the result throws out of type()', fn () => $throwing->type(), RuntimeException::class);
$GLOBALS['answer'] = static fn () => 'text/x-after-result';
$t->assertSame('nothing was kept after the result threw', $throwing->type(), 'text/x-after-result');

// The polyfill can return and have a local's destructor throw on the way out,
// which leaves both the result and the exception.
$GLOBALS['guard'] = true;
$GLOBALS['released'] = false;
$GLOBALS['answer'] = static fn () => new PolyfillResult();
$t->assertThrows('a local destructor throws after the polyfill returned', fn () => $req->file('doc')->type(), RuntimeException::class);
$t->assertTrue('the result is destroyed all the same', $GLOBALS['released']);
$GLOBALS['answer'] = static fn () => 'text/x-from-guarded-call';
$guarded = $req->file('doc');
$t->assertThrows('a string comes with that exception too', fn () => $guarded->type(), RuntimeException::class);
$GLOBALS['guard'] = false;
$GLOBALS['answer'] = static fn () => 'text/x-after-guard';
$t->assertSame('nothing was kept from that call', $guarded->type(), 'text/x-after-guard');

// A string from a polyfill is the detected type, kept like any other.
$GLOBALS['answer'] = static fn () => 'text/x-polyfill';
$detected = $req->file('doc');
$t->assertSame('a string result is returned', $detected->type(), 'text/x-polyfill');
$GLOBALS['answer'] = static fn () => 'text/x-not-read';
$t->assertSame('and kept on the object', $detected->type(), 'text/x-polyfill');

$t->done();
