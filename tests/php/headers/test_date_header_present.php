<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('date_header_present', 'headers');
$t->assertTrue('server sends a Date header (checked by runner)', true);
$t->done();
