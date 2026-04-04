<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('static_served', 'routing');
// Runner-side test: the runner requests a static asset in SPA mode and
// verifies it is served directly without falling back to the SPA page.
// This PHP file is a placeholder.
$t->assertTrue('file reached (runner validates static files served in SPA mode)', true);
$t->done();
