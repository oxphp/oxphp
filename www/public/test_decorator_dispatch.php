<?php

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
        $hasResult = $ctx->hasResult() ? 'yes' : 'no';
        echo "AFTER:{$this->tag}:result={$hasResult}\n";
    }
}

\oxphp_register_decorator(DebugDecorator::class);

// Test 1: Decorated function
#[DebugDecorator(label: 'fn')]
function decorated_function(): string {
    echo "EXEC:decorated_function\n";
    return "ok";
}

echo "--- Test 1: Function ---\n";
decorated_function();

// Test 2: Decorated method
class MyService {
    #[DebugDecorator(label: 'method')]
    public function doWork(): void {
        echo "EXEC:doWork\n";
    }
}

echo "--- Test 2: Method ---\n";
$svc = new MyService();
$svc->doWork();

// Test 3: Class-level decorator
#[DebugDecorator(label: 'class')]
class AuditedService {
    public function action1(): void {
        echo "EXEC:action1\n";
    }
    public function action2(): void {
        echo "EXEC:action2\n";
    }
}

echo "--- Test 3: Class ---\n";
$as = new AuditedService();
$as->action1();
$as->action2();

// Test 4: Multiple decorators (execution order)
#[Attribute(Attribute::TARGET_FUNCTION)]
class DecA implements OxPHP\Decorator\AttributeInterface {
    public function before(OxPHP\Decorator\Context $ctx): void { echo "A.before\n"; }
    public function after(OxPHP\Decorator\Context $ctx): void { echo "A.after\n"; }
}

#[Attribute(Attribute::TARGET_FUNCTION)]
class DecB implements OxPHP\Decorator\AttributeInterface {
    public function before(OxPHP\Decorator\Context $ctx): void { echo "B.before\n"; }
    public function after(OxPHP\Decorator\Context $ctx): void { echo "B.after\n"; }
}

oxphp_register_decorator(DecA::class);
oxphp_register_decorator(DecB::class);

#[DecA]
#[DecB]
function multi_decorated(): void {
    echo "EXEC:multi\n";
}

echo "--- Test 4: Order ---\n";
multi_decorated();

echo "ALL_TESTS_PASSED\n";
