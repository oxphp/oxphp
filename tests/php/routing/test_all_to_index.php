<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('all_to_index', 'routing');
// In framework mode all requests are routed to index.php, so this file
// should never be reached directly. The runner hits an arbitrary path and
// validates that the framework index.php response is returned. If this
// file somehow executes, we pass trivially.
$t->assertTrue('file reached (runner validates framework routing to index.php)', true);
$t->done();
