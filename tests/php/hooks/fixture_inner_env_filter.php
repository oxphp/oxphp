<?php

declare(strict_types=1);

// Inner self-request for hooks/test_env_filter_input ?mode=suspend. It can only
// be served while the outer request's fiber is parked in a hooked sleep, which
// is the whole point: what ext/filter reads for INPUT_ENV is one slot per worker
// thread, and a suspended request must not take it away with it. If the outer
// request's suspend snapshot carries that slot off, this request finds it
// undefined and every filter_*(INPUT_ENV) call here answers "no such variable".
//
// PATH rather than a fixture-specific name: the slot is a snapshot of the
// process environment, so the value has to come from the environment itself,
// and getenv() is the independent reader to compare against.
//
// Must not suspend — no await, no sleep, no socket read. This request exists to
// run to completion inside the outer one's window.
//
// Echo-style on purpose — the outer test asserts on this body, not on JSON.

$fail = [];

$path = getenv('PATH');
if (!is_string($path) || $path === '') {
    $fail[] = 'getenv(PATH) = ' . var_export($path, true) . ', expected a non-empty string';
}

if (filter_has_var(INPUT_ENV, 'PATH') !== true) {
    $fail[] = 'filter_has_var(INPUT_ENV, PATH) = false while another request is parked';
}

$one = filter_input(INPUT_ENV, 'PATH');
if ($one !== $path) {
    $fail[] = 'filter_input(INPUT_ENV, PATH) = ' . var_export($one, true)
        . ', expected ' . var_export($path, true);
}

$all = filter_input_array(INPUT_ENV);
if (!is_array($all) || ($all['PATH'] ?? null) !== $path) {
    $fail[] = 'filter_input_array(INPUT_ENV) did not carry PATH: '
        . var_export(is_array($all) ? ($all['PATH'] ?? null) : $all, true);
}

$meta = json_encode(['tag' => $_GET['tag'] ?? '?']);

if ($fail !== []) {
    http_response_code(500);
    echo "INNER-FAIL $meta: " . implode('; ', $fail) . "\n";
    return;
}

echo "INNER-OK $meta\n";
