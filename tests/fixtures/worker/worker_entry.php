<?php
static $requestCount = 0;
static $previousHeaders = [];

// A per-worker store for tests that need one object shared by every request the
// worker serves. That is the shape a real application has: WordPress, Laravel
// and Symfony build their database and cache clients once when the worker boots
// and hand the same ones to every request, so a client's connection is process
// state, not request state. Included test files reach it because PHP `include`
// runs in the includer's scope.
static $sharedState = [];

// Captured during the worker boot phase (before oxphp_worker enters its
// receive loop). With the request_time consistency fix these values must
// both be exactly 0.0 because no request is being processed yet. Passed
// into the closure so included test files can assert on them directly
// (PHP `include` runs in the includer's scope).
$bootInfo = [
    'request_time'       => oxphp_server_info()['request_time'],
    'request_start_time' => oxphp_http_request()->startTime(true),
];

// An ini directive set during the worker boot phase, for the test that a
// request's own ini_set() is undone against what boot left rather than against
// the engine default. user_agent is PHP_INI_ALL and is read only by the http://
// stream wrapper, so no other test here depends on its value.
//
// The value is computed rather than written as a literal on purpose: a literal
// is interned and lives outside the request heap, while a computed string is
// allocated in it — and the second is the case a worker has to carry past its
// own request teardown.
//
// The four values are captured rather than hardcoded in the test: what this
// worker booted with is the only correct baseline, whatever php.ini the image
// carries. Passed into the closure so included test files can read it directly
// (PHP `include` runs in the includer's scope).
ini_set('user_agent', 'oxphp-boot-probe-' . getmypid());
$iniBoot = [
    'user_agent'         => ini_get('user_agent'),
    'precision'          => ini_get('precision'),
    'display_errors'     => ini_get('display_errors'),
    'max_execution_time' => ini_get('max_execution_time'),
];

// A directive whose handler treats the startup stage differently from every
// other one: it keeps the value it is given there as the floor and afterwards
// refuses any change that relaxes it. Read first, then tightened, so that
// worker/test_phar_readonly_floor can tell its own two failure modes apart —
// a floor of 1 that came from php.ini is the profile missing its ini mount,
// while a floor of 1 under php.ini's 0 is the bug the test is there for.
//
// Tightening at bootstrap and relaxing around the one path that writes a phar
// is what an application with such a path does. Only the profile that sets the
// environment variable does it, so no other worker profile is affected.
//
// The value comes from the environment rather than from a literal for the same
// reason the user_agent probe above is computed: a literal is interned and
// lives outside the request heap, so the copy that carries bootstrap values
// past their request would skip it and the handler would never run again — the
// exact step this directive must not take.
$pharReadonlyPhpIni = ini_get('phar.readonly');
// An empty variable counts as absent rather than as a value: getenv() answers
// '' for a variable that is set and empty, and '' parses as 0 — so taking it
// would have the bootstrap relax the directive instead of tightening it, and
// the test would then fail saying the bootstrap did not run.
$pharReadonlyBoot = getenv('OXPHP_TEST_PHAR_READONLY');
if ($pharReadonlyBoot !== false && $pharReadonlyBoot !== '') {
    ini_set('phar.readonly', $pharReadonlyBoot);
}

oxphp_worker(function () use (&$requestCount, &$previousHeaders, &$sharedState, $bootInfo, $iniBoot, $pharReadonlyPhpIni) {
    $requestCount++;

    // If the request targets a test PHP file, include it directly inside
    // the worker callback so tests run in real worker context.
    $uri = parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH);
    if (preg_match('#^/tests/.+\.php$#', $uri)) {
        $testFile = $_SERVER['DOCUMENT_ROOT'] . $uri;
        if (file_exists($testFile)) {
            include $testFile;
            return;
        }
    }

    header('Content-Type: application/json');

    $action = $_GET['action'] ?? 'default';

    $response = match ($action) {
        'is_worker'       => ['is_worker' => oxphp_is_worker()],
        'state_persists'  => ['request_count' => $requestCount],
        'superglobals'    => ['get' => $_GET, 'server' => $_SERVER['REQUEST_METHOD'] ?? ''],
        'check_output'    => ['clean' => true],
        'check_headers'   => [
            'prev' => $previousHeaders,
            'current_id' => oxphp_request_id(),
        ],
        'server_info'     => oxphp_server_info(),
        default           => ['action' => $action, 'request_count' => $requestCount],
    };

    $previousHeaders = headers_list();

    echo json_encode($response, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES);
});
