<?php

declare(strict_types=1);

require_once '/srv/lib/test_helper.php';

/**
 * What the entry sees with DOCUMENT_ROOT switched off, whichever URI the
 * request was sent to; the runner names that URI in X-Expect-Uri.
 */
function entry_only_check(string $entry): void
{
    $name = '/' . basename($entry);
    $t = new TestCase('entry_only_request', 'entry_only');
    $t->assertSame(
        'REQUEST_URI',
        $_SERVER['REQUEST_URI'] ?? '',
        $_SERVER['HTTP_X_EXPECT_URI'] ?? '(no X-Expect-Uri)'
    );
    $t->assertSame('DOCUMENT_ROOT', $_SERVER['DOCUMENT_ROOT'] ?? '', dirname($entry));
    $t->assertSame('SCRIPT_FILENAME', $_SERVER['SCRIPT_FILENAME'] ?? '', $entry);
    $t->assertSame('SCRIPT_NAME', $_SERVER['SCRIPT_NAME'] ?? '', $name);
    $t->assertSame('PHP_SELF', $_SERVER['PHP_SELF'] ?? '', $name);
    $t->assertKeyMissing('PATH_INFO', $_SERVER, 'PATH_INFO');
    $t->done();
}
