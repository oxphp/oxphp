<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('scheme_method', 'tls');
$req = oxphp_http_request();
$t->assertSame('scheme() is "https"', $req->scheme(), 'https');
$t->done();
