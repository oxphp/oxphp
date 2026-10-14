<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A script may still include a file opcache.preload loaded. Preloading moves
// the file's functions and classes out of it and does not count the file as
// included, so require_once runs the file's top-level statements again —
// it returns 1, not true — and declares nothing a second time.
$t = new TestCase('test_preloaded_file_required_again', 'asyncdecl');

$returned = require_once __DIR__ . '/lib.php';

$t->assertSame('require_once runs the file again', $returned, 1);
$t->assertSame('the preloaded function answers', asyncdecl_pre_helper(), 11);
$t->assertSame('the preloaded class answers', (new AsyncDeclPreNamed())->x, 12);

$t->done();
