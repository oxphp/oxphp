<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Past max_input_vars the parser warns and stops, so what it has gathered is only
// part of the body. A handler that throws from that warning must leave none of it
// behind, or the next call would return it as if the whole body had parsed.
$t = new TestCase('payload_put_max_input_vars', 'http_object');

$limit = (int) ini_get('max_input_vars');
$req = oxphp_http_request();
$t->assertSame('the body has one field past the limit', substr_count($req->body(), '&') + 1, $limit + 1);

set_error_handler(static function (int $errno, string $errstr): never {
    throw new ErrorException($errstr, 0, $errno);
});
$call = static function () use ($req): string {
    try {
        return 'returned ' . count($req->payload()) . ' fields';
    } catch (ErrorException $e) {
        return str_contains($e->getMessage(), 'max_input_vars') ? 'threw the limit warning' : $e->getMessage();
    }
};
$t->assertSame('a throwing handler stops the call', $call(), 'threw the limit warning');
$t->assertSame('and the next call on that object is not served a partial result', $call(), 'threw the limit warning');
restore_error_handler();

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = [$errno, $errstr];
    return true;
});
$req = oxphp_http_request();
$fields = $req->payload();
$again = $req->payload();
restore_error_handler();

$t->assertSame('the call that reaches the limit warns once', count($warnings), 1);
$t->assertSame('with E_WARNING', $warnings[0][0] ?? null, E_WARNING);
$t->assertTrue('naming the setting', str_contains($warnings[0][1] ?? '', 'max_input_vars'));
$t->assertSame('parsing stops at the limit', count($fields), $limit);
$t->assertSame('keeping the last field inside it', $fields['k' . ($limit - 1)] ?? null, 'v' . ($limit - 1));
$t->assertFalse('and dropping the one past it', array_key_exists('k' . $limit, $fields));
$t->assertSame('a handler that returns leaves a result to cache', $again, $fields);

$t->done();
