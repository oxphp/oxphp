<?php

declare(strict_types=1);

// Included twice by fixture_lookup_spl_stack_twice.php, and built the same way
// as fixture_lookup_array_object.php around another class.
return oxphp_async_await(oxphp_async(static function (?splStack $unused = null): string {
    return get_class(new splStack());
}));
