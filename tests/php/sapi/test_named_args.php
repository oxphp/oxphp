<?php

require __DIR__ . '/../test_helper.php';

$t = new TestCase('named_args', 'sapi');

$ch = new OxPHP\Shared\Channel(1);

$rm = new ReflectionMethod($ch, 'send');
$names = array_map(fn($p) => $p->getName(), $rm->getParameters());
$t->assertSame('send param 0 name', $names[0] ?? null, 'value');
$t->assertSame('send param 1 name', $names[1] ?? null, 'timeout');

// Functional: must accept named arg without throwing.
$ch->send('x', timeout: 0.0);
$t->assertSame('value passed through', $ch->recv(), 'x');

$t->done();
