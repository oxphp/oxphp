<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// payload() answers with the body as the client sent it, like every other reader
// on this object. Writing to $_POST changes what the script sees and nothing else.
$t = new TestCase('payload_post_as_sent', 'http_object');

$req = oxphp_http_request();

$_POST['before'] = 'written before the first read';
$t->assertSame('$_POST holds the write', $_POST['before'] ?? null, 'written before the first read');
$t->assertSame('payload() ignores a write made before it', $req->payload(), ['a' => '1']);

$_POST['after'] = 'written after the first read';
$t->assertSame('and one made after it', $req->payload(), ['a' => '1']);
$t->assertSame('a new object reads the body too', oxphp_http_request()->payload(), ['a' => '1']);

$_POST = ['replaced' => '1'];
$t->assertSame('replacing $_POST changes nothing', oxphp_http_request()->payload(), ['a' => '1']);

$t->done();
