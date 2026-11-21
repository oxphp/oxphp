<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_class_lookup_slot_after_an_included_file_fails_to_compile_on_the_pool_thread', 'asyncnocache');

// A file a task includes is compiled on the async thread as well: a fatal error
// that ends that compile gives up the numbers it gave the class names it met,
// as a finished compile does. The file is written here, so that no script that
// fails to compile sits in the document root.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$file = sys_get_temp_dir() . '/oxphp-include-compile-error-' . bin2hex(random_bytes(4)) . '.php';
file_put_contents(
    $file,
    '<?php $f = static fn (?SPlSTack $a = null, ?SpLSTack $b = null, ?SplSTAck $c = null, ?SplStACk $d = null,'
    . ' ?SplStaCK $e = null, ?SPlStacK $f = null, ?SpLStacK $g = null, ?SPLStacK $h = null): int => 0;'
    . ' abstract class OxphpAsyncnocacheIncludeCompileError { abstract public function f() {} }',
);

$failure = '';
try {
    oxphp_async_await(oxphp_async(static function () use ($file): void {
        include $file;
    }));
} catch (\Throwable $e) {
    $failure = $e->getMessage();
}
@unlink($file);
$t->assertContains('the included file ends in a fatal error', $failure, 'cannot contain body');

$t->assertSame(
    'a task looks up classes through the names that compile met',
    oxphp_async_await(oxphp_async(static fn (): array => eval(
        'return array_map(get_class(...), [new SPlSTack(), new SpLSTack(), new SplSTAck(), new SplStACk(),'
        . ' new SplStaCK(), new SPlStacK(), new SpLStacK(), new SPLStacK()]);'
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
