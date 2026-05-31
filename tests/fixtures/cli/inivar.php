<?php
// max_input_vars is PHP_INI_PERDIR: a runtime zend_alter_ini(ZEND_INI_USER)
// cannot change it, so `-d max_input_vars=...` only takes effect when folded
// into the ini_entries blob (config stage). Proves -d covers non-ALL directives.
echo ini_get('max_input_vars'), "\n";
