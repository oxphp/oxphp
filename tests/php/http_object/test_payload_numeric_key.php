<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// JSON objects and form fields both store a canonical numeric name under an
// integer key, so a lookup that only searches string keys misses a key that
// payload() returns in full. "01" is not canonical and stays a string key.
$t = new TestCase('payload_numeric_key', 'http_object');

$req = oxphp_http_request();

$t->assertSame('payload() holds "0" under an integer key', $req->payload()[0] ?? null, 'a');
$t->assertSame('payload("0")', $req->payload('0', '<DEFAULT>'), 'a');
$t->assertSame('payload("12")', $req->payload('12', '<DEFAULT>'), 'b');
$t->assertSame('payload("-1")', $req->payload('-1', '<DEFAULT>'), 'c');
$t->assertSame('payload("01") is a string key', $req->payload('01', '<DEFAULT>'), 'd');
$t->assertSame('payload("name")', $req->payload('name', '<DEFAULT>'), 'n');
$t->assertSame('an absent numeric key gives the default', $req->payload('99', '<DEFAULT>'), '<DEFAULT>');

$t->done();
