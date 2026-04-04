<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('full_uri_https', 'tls');
$req = oxphp_http_request();
$uri = $req->fullUri();
$t->assertTrue('fullUri() starts with "https://"', str_starts_with($uri, 'https://'));
$t->done();
