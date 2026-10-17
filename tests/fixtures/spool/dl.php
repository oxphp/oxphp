<?php
// The pattern from the field: a protected download handed to PHP. readfile()
// maps the whole file and writes it to the SAPI in one call.
$f = '/tmp/big.bin';
header('Content-Type: application/octet-stream');
header('Content-Length: ' . filesize($f));
readfile($f);
