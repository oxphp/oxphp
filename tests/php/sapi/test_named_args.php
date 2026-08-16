<?php

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('named_args', 'sapi');

$ch = new OxPHP\Shared\Channel(1);

// sendTimeout exposes the int $ms parameter by name (Channel::send is
// the no-timeout variant under the new Result API).
$rm = new ReflectionMethod($ch, 'sendTimeout');
$names = array_map(fn($p) => $p->getName(), $rm->getParameters());
$t->assertSame('sendTimeout param 0 name', $names[0] ?? null, 'value');
$t->assertSame('sendTimeout param 1 name', $names[1] ?? null, 'ms');

// Functional: must accept named arg without throwing.
$result = $ch->sendTimeout('x', ms: 50);
$t->assertSame('sendTimeout returns Ok', $result->isOk(), true);
$t->assertSame('value passed through', $ch->recv()->value(), 'x');

$t->done();
