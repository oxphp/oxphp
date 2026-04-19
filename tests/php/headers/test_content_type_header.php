<?php
// Runner-side test: validates that a PHP-set Content-Type survives to the
// client. Does NOT use TestCase/done() because test_helper.php::output()
// forces Content-Type: application/json, which would mask the real value.
// The runner asserts on the response header only.
header('Content-Type: text/plain');
echo "ok";
