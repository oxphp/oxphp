<?php
// A run default php.ini cannot override, then one it can.
echo ini_get('max_execution_time'), "|", ini_get('display_errors'), "\n";
