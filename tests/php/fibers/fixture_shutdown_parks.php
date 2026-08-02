<?php

declare(strict_types=1);

// Inner self-request for fibers/test_shutdown_function_survives_suspend.
//
// Requested twice by that test: once with park=1, which registers a shutdown
// function and then parks in a hooked sleep, and once without, which registers
// one of its own and ends inside the window the first one is parked for.
//
// Each request's shutdown function echoes the id of the request that registered
// it, and the end of a request runs its shutdown functions into that request's
// response — so a body carrying an id that is not its own is one request running
// another request's shutdown functions, and a body missing its own is a request
// that lost them while it was parked.

$id = (string) ($_GET['id'] ?? '?');
$park = ($_GET['park'] ?? '0') === '1';

header('Content-Type: text/plain');

register_shutdown_function(static function () use ($id): void {
    echo "SHUTDOWN-RAN:{$id}\n";
});

echo "ARMED:{$id}\n";

if ($park) {
    // Hooked: parks this request's fiber, which is what frees the worker to
    // serve the second request while this one still holds a registration.
    sleep(1);
    echo "RESUMED:{$id}\n";
}
