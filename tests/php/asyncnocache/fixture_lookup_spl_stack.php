<?php

declare(strict_types=1);

// Fetched by test_task_class_lookup_slot_of_another_request after
// fixture_lookup_array_object, and built the same way around another class.
$task = oxphp_async(static function (?splStack $unused = null): string {
    return get_class(new splStack());
});

echo oxphp_async_await($task);
