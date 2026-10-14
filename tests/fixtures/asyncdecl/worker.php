<?php

declare(strict_types=1);

// Worker entry for the asyncdecl profile. The worker thread runs this file and
// the async pool's threads never do, so what it declares exists only on the
// worker thread. Test files are included from the callback below: they run on
// the worker thread too, and see $fixtures.

// The typed parameter keeps OPcache's optimizer from inlining calls to it:
// a call to a function that only returns a constant is replaced by that
// constant, and nothing would be left to call.
function asyncdecl_helper(int $base = 0): int
{
    return $base + 7;
}

final class AsyncDeclNamed
{
    public int $x = 8;
}

// The shapes the crash was first reported with, compiled here as they were.
// Each closure comes with the line it is on.
$fixtures = [
    // A call to a function of this file.
    'call' => [__LINE__, static fn (): int => asyncdecl_helper()],
    'anon' => [__LINE__, static fn (): int => (new class { public int $x = 9; })->x],
    // A named class is looked up with autoloading, which fails with an Error.
    'named' => static fn (): int => (new AsyncDeclNamed())->x,
    // A pool whose factory builds an anonymous class.
    'pool' => [__LINE__ + 2, new \OxPHP\Shared\Pool(
        factory: static fn (): object
            => new class (random_int(1, 100)) extends \stdClass {
                public function __construct(public int $n)
                {
                }
            },
        destroy: null,
        maxSize: 2,
    )],
];

oxphp_worker(function () use ($fixtures): void {
    $uri = parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH);
    if (is_string($uri) && preg_match('#^/tests/.+\.php$#', $uri)) {
        $testFile = $_SERVER['DOCUMENT_ROOT'] . $uri;
        if (is_file($testFile)) {
            include $testFile;
            return;
        }
    }
    http_response_code(404);
});
