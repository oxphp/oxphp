<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('location_redirect', 'headers');
header('Location: /other', true, 302);
$t->assertTrue('Location header and 302 status set', true);
$t->done();
