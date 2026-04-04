<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('static_still_served', 'routing');
// Runner-side test: the runner requests a static asset in framework mode
// and verifies it is served correctly. This PHP file is a placeholder.
$t->assertTrue('file reached (runner validates static files served in framework mode)', true);
$t->done();
