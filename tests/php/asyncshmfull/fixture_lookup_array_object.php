<?php

declare(strict_types=1);

// Included twice by test_task_class_lookup_slot_of_a_script_from_the_file_cache.
// The class is not spelled the way PHP registered it, so loading the script from
// the file cache into the memory of the request gives the name a class-lookup
// slot that PHP thread numbers itself; the type and `new` are one string there.
return oxphp_async_await(oxphp_async(static function (?arrayObject $unused = null): string {
    return get_class(new arrayObject());
}));
