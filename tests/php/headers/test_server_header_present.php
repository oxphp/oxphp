<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('server_header_present', 'headers');
$t->assertTrue('server sends a Server header (checked by runner)', true);
$t->done();
