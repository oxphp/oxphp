<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('missing_to_html', 'routing');
// Runner-side test: in SPA mode a request for a nonexistent path should
// return the SPA fallback HTML page. The runner hits a nonexistent URL and
// checks the response body for the fallback content. This PHP file is a
// placeholder and should not execute for that request.
$t->assertTrue('file reached (runner validates missing path returns SPA fallback)', true);
$t->done();
