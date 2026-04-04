<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('context_properties', 'decorators');

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class ContextInspector implements \OxPHP\Decorator\AttributeInterface
{
    public static ?\OxPHP\Decorator\Context $captured = null;

    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        self::$captured = $ctx;
    }

    public function after(\OxPHP\Decorator\Context $ctx): void {}
}

oxphp_register_decorator(ContextInspector::class);

#[ContextInspector]
function context_target(): void {}

context_target();

$ctx = ContextInspector::$captured;
$t->assertNotNull('context was captured', $ctx);
$t->assertNotEmpty('context has non-empty requestId', $ctx->requestId);
$t->assertNotEmpty('context has non-empty target name', $ctx->target);
$t->assertSame('context target is context_target', $ctx->target, 'context_target');

$t->done();
