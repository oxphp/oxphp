<?php

declare(strict_types=1);

// Fetched by test_task_class_lookup_slot_of_another_request. The parameter's
// type is the only class name in this file, and it is not spelled the way PHP
// registered the class, so the request interns it and gives it a class-lookup
// slot of its own.
$task = oxphp_async(static function (?arrayObject $unused = null): string {
    return get_class(new arrayObject());
});

echo oxphp_async_await($task);
