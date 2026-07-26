<?php
// $_ENV must survive every request on a worker, whatever touches it.
//
// $_ENV describes the process, not the request, and its auto-global callback
// does not refresh the array — it destroys it and repopulates from the process
// environment. Applications put things there that the environment has never
// heard of: a .env loader writes its values straight into $_ENV (and $_SERVER)
// without calling putenv(), and in worker mode it runs once at boot and never
// again. Rebuilding $_ENV therefore erases the application's configuration.
//
// Not forcing the callback is not enough to prevent that, which is what the
// middle request here is for. Every reset re-arms the lazy auto-globals, so any
// zend_is_auto_global() for _ENV from anywhere fires the callback: compiling a
// file that mentions $_ENV, OPcache pinging one it loads from cache, or
// filter_input(INPUT_ENV, ...) and its siblings. The reset closes that by
// disarming _ENV once the array exists, and these three requests check it:
// request one seeds a key the way a .env loader does, request two compiles a
// mention of $_ENV, request three must still see the key.
//
// eval() is that trigger in its most deterministic form — it compiles on every
// request no matter what OPcache holds, so the middle line pulls the same lever
// as a real application loading its first $_ENV-reading class, without
// depending on cache state. That also keeps the test honest with OPcache off:
// the recompile of this very file stops mattering once _ENV is disarmed.
//
// All three must stay adjacent and on one worker, which is why this is listed
// only in the profile that pins PHP_WORKERS=1; in a multi-worker pool a miss
// would be indistinguishable from the defect.
//
// The guarantee assumes the default auto_globals_jit=1. With it off, _ENV is not
// a lazy auto-global at all: zend_activate_auto_globals() rebuilds it from the
// process environment on every request, before anything can disarm it. This test
// would catch that — correctly, since the application's $_ENV really is
// discarded in that configuration.
//
// Written without test_helper.php: the tests under tests/php/worker pull it in
// with a bare `require`, so a worker serving two of them fatals on the class
// redeclare.

$fail = [];
$mode = $_GET['mode'] ?? '';

if ($mode === 'seed') {
    $_ENV['OX_ENV_PROBE'] = 'seeded';
    if (($_ENV['OX_ENV_PROBE'] ?? null) !== 'seeded') {
        $fail[] = 'write to $_ENV was not visible within the seeding request itself';
    }
} elseif ($mode === 'touch') {
    // Compiling this mention of $_ENV is the whole point of the request: on an
    // armed _ENV it calls php_auto_globals_create_env(), which discards what the
    // seed request wrote. A fixed literal, and the compile step is the lever
    // being pulled — nothing here is derived from input.
    eval('$ignored = $_ENV;');
} else {
    $probe = $_ENV['OX_ENV_PROBE'] ?? null;
    if ($probe !== 'seeded') {
        $fail[] = '$_ENV[OX_ENV_PROBE] = ' . var_export($probe, true)
            . ", expected 'seeded' — the value an earlier request on this worker"
            . ' put there. Something fired the _ENV callback in between (the'
            . ' preceding request compiles a mention of $_ENV, which does exactly'
            . ' that while _ENV is armed), so the array was rebuilt from the'
            . ' process environment and those values are gone';
    }
}

if ($fail !== []) {
    http_response_code(500);
    echo 'FAIL: ' . implode('; ', $fail) . "\n";
    return;
}

echo "OK mode=$mode\n";
