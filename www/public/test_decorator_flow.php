<?php

// Test 1: Registration works
#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD | Attribute::TARGET_CLASS)]
class DebugDecorator implements OxPHP\Decorator\AttributeInterface {
    private string $tag;

    public function __construct(public readonly string $label = 'default') {
        $this->tag = '';
    }

    public function before(OxPHP\Decorator\Context $ctx): void {
        $this->tag = $this->label . ':' . $ctx->target;
        echo "BEFORE:{$this->tag}\n";
    }

    public function after(OxPHP\Decorator\Context $ctx): void {
        echo "AFTER:{$this->tag}\n";
    }
}

$result = oxphp_register_decorator(DebugDecorator::class);
echo "REGISTERED:" . ($result ? "true" : "false") . "\n";

// Test 2: Registration validation
$result2 = oxphp_register_decorator('NonExistentClass');
echo "NON_EXISTENT:" . ($result2 ? "true" : "false") . "\n";

// Test 3: Context class works
$ctx = new OxPHP\Decorator\Context();
echo "CONTEXT_CLASS:" . get_class($ctx) . "\n";
echo "HAS_RESULT:" . ($ctx->hasResult() ? "true" : "false") . "\n";

// Test 4: RejectedException exists
try {
    throw new OxPHP\Decorator\RejectedException("test rejection");
} catch (OxPHP\Decorator\RejectedException $e) {
    echo "REJECTED:" . $e->getMessage() . "\n";
}

echo "ALL_TESTS_PASSED\n";
