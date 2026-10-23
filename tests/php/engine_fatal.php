<?php

declare(strict_types=1);

// A fatal error for the fixtures that need a request to end in one: a real
// zend_bailout, which leaves the frames it was raised from standing for the
// worker to deal with, where an exception would unwind them on its way out.
//
// Not trigger_error() with E_USER_ERROR. As of PHP 8.4 that raises an
// E_DEPRECATED before the fatal, and an error handler that throws on it — the
// one TestCase installs, which a worker keeps across requests, so it is still
// standing on an inner request a TestCase test sends — makes trigger_error()
// return with the exception, never raising the fatal. Even without the
// deprecation, E_USER_ERROR is handed to a user error handler, which can take it
// over.
//
// A compile error is handed to no error handler: the engine reports
// E_COMPILE_ERROR and bails out. The class fails to compile before it is bound,
// so nothing is declared and the same $tag can fatal again on the next request.
// The message names it: "Abstract function <tag>::f() cannot contain body".
//
// What that costs: the compile is abandoned partway, and what the compiler had
// allocated for it — its syntax-tree arena alone is 32 KiB — is given back with
// the rest of the request's heap, which on a worker is when the worker goes.
// Each call leaves about 39 KB behind until then. Nothing for a fixture about
// what a fatal does; wrong for one measured by what a fatal leaves behind,
// which is why fibers/fixture_fatal.php fatals another way.
//
// Pulled in with require_once and declared conditionally for the reason
// fibers/fixture_closure_fatal.php gives.

if (!function_exists('engine_fatal')) {
    function engine_fatal(string $tag): never
    {
        eval("abstract class {$tag} { abstract public function f() {} }");
    }
}
