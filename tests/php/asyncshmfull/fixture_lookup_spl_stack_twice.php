<?php

declare(strict_types=1);

// Fetched by test_task_class_lookup_slot_of_a_script_from_the_file_cache, on
// the other PHP worker: the first include compiles the fixture and stores it in
// the file cache, the second loads it from there.
$first = include __DIR__ . '/fixture_lookup_spl_stack.php';
$second = include __DIR__ . '/fixture_lookup_spl_stack.php';
echo "$first $second";
