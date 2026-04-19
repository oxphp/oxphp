<?php
// Runner-side test: validates header_remove() followed by a re-add leaves
// only the new value visible to the client. Does NOT use TestCase/done()
// because test_helper.php::output() forces a single JSON content-type,
// and the assertion lives on the response header itself.
header('X-Cycle: v1');
header_remove('X-Cycle');
header('X-Cycle: v2');
echo "ok";
