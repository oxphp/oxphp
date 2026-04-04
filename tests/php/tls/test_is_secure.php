<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('is_secure', 'tls');
$req = oxphp_http_request();
$t->assertTrue('isSecure() is true', $req->isSecure());
$t->done();
