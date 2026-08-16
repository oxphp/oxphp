<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('custom_error_page', 'errors');
// This script itself is a pass placeholder.
// The runner tests the custom 404 page by hitting a nonexistent URL and checking
// that the response body contains the string "CUSTOM_404_PAGE".
$t->assertTrue('placeholder — runner checks custom 404 page content', true);
$t->done();
