<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A canonical decimal name is an integer key in a PHP array, so the array all()
// hands out has to hold it as one for PHP code to find it there.
$t = new TestCase('attributes_numeric_name', 'http_object');

$attrs = oxphp_http_request()->attributes();

$attrs->set('7', 'x');
$t->assertSame('get("7")', $attrs->get('7'), 'x');
$t->assertTrue('has("7")', $attrs->has('7'));
$t->assertSame('all() keys "7" as the integer 7', array_keys($attrs->all()), [7]);
$t->assertKeyExists('all() finds "7"', $attrs->all(), '7');
$t->assertSame('all()[7]', $attrs->all()[7] ?? 'MISS', 'x');

$attrs->set('7', 'y');
$t->assertSame('a second set("7") replaces the value', $attrs->all(), [7 => 'y']);

$attrs->remove('7');
$t->assertFalse('remove("7") clears has()', $attrs->has('7'));
$t->assertSame('remove("7") empties all()', $attrs->all(), []);

// A name PHP does not read as an integer stays a string key.
$attrs->set('07', 'z');
$t->assertSame('"07" stays a string key', array_keys($attrs->all()), ['07']);
$t->assertSame('get("07")', $attrs->get('07'), 'z');
$t->assertSame('get("7") does not reach "07"', $attrs->get('7', 'dflt'), 'dflt');

$attrs->remove('07');
$attrs->set('9223372036854775808', 'o');
$t->assertSame('a name past PHP_INT_MAX stays a string key', array_keys($attrs->all()), ['9223372036854775808']);
$t->assertSame('get() of a name past PHP_INT_MAX', $attrs->get('9223372036854775808'), 'o');

$t->done();
