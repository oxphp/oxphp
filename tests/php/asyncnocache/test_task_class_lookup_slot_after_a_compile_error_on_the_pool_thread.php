<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_class_lookup_slot_after_a_compile_error_on_the_pool_thread', 'asyncnocache');

// A compile the async thread runs for a task numbers the class names it meets
// before a fatal error ends it, and the thread lives on. The names in code
// another PHP thread compiled carry numbers from the same start, so the slots
// this thread fills for its own names must not become theirs: the fatal gives
// the numbers up as a finished compile does.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$failure = '';
try {
    oxphp_async_await(oxphp_async(static function (): void {
        eval(
            '$f = static fn (?SPlStack $a = null, ?SpLStack $b = null, ?SplSTack $c = null, ?SplStAck $d = null,'
            . ' ?SplStaCk $e = null, ?SplStacK $f = null, ?SPLStack $g = null, ?SplSTACK $h = null): int => 0;'
            . ' abstract class OxphpAsyncnocacheCompileError { abstract public function f() {} }'
        );
    }));
} catch (\Throwable $e) {
    $failure = $e->getMessage();
}
$t->assertContains('the compile ends in a fatal error', $failure, 'cannot contain body');

$t->assertSame(
    'a task looks up classes through the names that compile met',
    oxphp_async_await(oxphp_async(static fn (): array => eval(
        'return array_map(get_class(...), [new SPlStack(), new SpLStack(), new SplSTack(), new SplStAck(),'
        . ' new SplStaCk(), new SplStacK(), new SPLStack(), new SplSTACK()]);'
    ))),
    array_fill(0, 8, 'SplStack'),
);

$get = static fn (string $fixture): string|false => @file_get_contents(
    "http://127.0.0.1/tests/asyncnocache/$fixture.php",
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);

$t->assertSame('the next task builds its own class', $get('fixture_lookup_array_object'), 'ArrayObject');
$t->assertSame('and so does the one after it', $get('fixture_lookup_spl_stack'), 'SplStack');

$t->done();
