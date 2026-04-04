<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('return_types', 'async');

// null
$p = oxphp_async(function(): mixed { return null; });
$t->assertSame('null return', oxphp_async_await($p), null);

// bool true
$p = oxphp_async(function(): bool { return true; });
$t->assertSame('bool true return', oxphp_async_await($p), true);

// bool false
$p = oxphp_async(function(): bool { return false; });
$t->assertSame('bool false return', oxphp_async_await($p), false);

// int
$p = oxphp_async(function(): int { return 42; });
$t->assertSame('int return', oxphp_async_await($p), 42);

// float
$p = oxphp_async(function(): float { return 3.14; });
$r = oxphp_async_await($p);
$t->assertTrue('float return within epsilon', abs($r - 3.14) < 1e-10);

// string
$p = oxphp_async(function(): string { return 'hello'; });
$t->assertSame('string return', oxphp_async_await($p), 'hello');

// array
$p = oxphp_async(function(): array { return [1, 2, 3]; });
$t->assertSame('array return', oxphp_async_await($p), [1, 2, 3]);

// empty string
$p = oxphp_async(function(): string { return ''; });
$t->assertSame('empty string return', oxphp_async_await($p), '');

// zero
$p = oxphp_async(function(): int { return 0; });
$t->assertSame('zero return', oxphp_async_await($p), 0);

$t->done();
