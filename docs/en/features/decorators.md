---
title: Attribute-Based Decorators
description: Intercept PHP function and method calls using PHP 8+ attributes
---

OxPHP provides an attribute-based decorator system that intercepts PHP function and method calls at the engine level. Decorators use the PHP 8+ Observer API (`zend_observer_fcall`) for zero-overhead interception of undecorated functions and transparent wrapping of decorated ones.

The system provides only the **interception mechanism**. What decorators do (timing, metrics, circuit breaking, caching) is the responsibility of the decorator implementation.

## How It Works

1. A PHP class implements `OxPHP\Decorator\AttributeInterface` and is marked with `#[Attribute]`
2. The class is registered with OxPHP via `oxphp_register_decorator()`
3. When the attribute is placed on a function, method, or class, OxPHP intercepts every call
4. The decorator's `before()` and `after()` methods fire around the original function

```php
#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements OxPHP\Decorator\AttributeInterface {
    private float $startTime;

    public function __construct(
        public readonly string $label = '',
    ) {}

    public function before(OxPHP\Decorator\Context $ctx): void {
        $this->startTime = hrtime(true);
    }

    public function after(OxPHP\Decorator\Context $ctx): void {
        $elapsed = hrtime(true) - $this->startTime;
        error_log(sprintf('[Timer] %s: %.2fms', $this->label ?: $ctx->target, $elapsed / 1e6));
    }
}

// Register once at application bootstrap
oxphp_register_decorator(Timer::class);
```

Once registered, use the attribute on any function or method:

```php
#[Timer(label: 'get_user')]
function getUser(int $id): User {
    return $db->find($id);
}

class OrderService {
    #[Timer(label: 'place_order')]
    public function placeOrder(array $items): Order {
        // ...
    }
}
```

## PHP API

### `OxPHP\Decorator\AttributeInterface`

The interface that all decorator classes must implement:

```php
namespace OxPHP\Decorator;

interface AttributeInterface {
    public function before(Context $ctx): void;
    public function after(Context $ctx): void;
}
```

### `OxPHP\Decorator\Context`

Read-only context object passed to `before()` and `after()`:

| Property | Type | Description |
|----------|------|-------------|
| `$target` | `string` | Full target name (`App\Service::method` or `my_function`) |
| `$class` | `string` | Class name, or `""` for functions |
| `$method` | `string` | Method name, or `""` for functions |
| `$function` | `string` | Function name for `TARGET_FUNCTION`, or `""` for methods |
| `$objectId` | `int` | `spl_object_id` for methods, `0` for functions |
| `$requestId` | `string` | Current request ID |
| `$traceId` | `string` | W3C trace ID (if tracing enabled) |

| Method | Return | Description |
|--------|--------|-------------|
| `getParams()` | `array` | Arguments passed to the decorated function (lazy, zero cost if not called) |
| `getResult()` | `mixed` | Return value of the decorated function (only in `after()`, returns `null` in `before()`) |
| `hasResult()` | `bool` | `true` in `after()` when the function returned successfully, `false` otherwise |

### `OxPHP\Decorator\RejectedException`

Exception thrown when a Rust-native decorator rejects a call via `DecoratorAction::Reject`. Extends `\Exception`.

### `oxphp_register_decorator()`

```php
oxphp_register_decorator(string $class): bool
```

Registers a PHP class as a decorator. The class must implement `OxPHP\Decorator\AttributeInterface` and be marked with `#[Attribute(...)]`. Returns `true` on success, `false` with an `E_WARNING` on validation failure.

## Attribute Targets

Decorator attributes are not required to support all targets. Each decorator class declares its own targets via PHP's `#[Attribute(...)]`:

```php
// Methods only
#[Attribute(Attribute::TARGET_METHOD)]
class RequireAuth implements OxPHP\Decorator\AttributeInterface { ... }

// Classes only — before()/after() fires on every method of the class
#[Attribute(Attribute::TARGET_CLASS)]
class Audited implements OxPHP\Decorator\AttributeInterface { ... }

// All three
#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements OxPHP\Decorator\AttributeInterface { ... }
```

PHP validates targets at compile time. Placing an attribute where its target flags don't allow produces a PHP error before any decorator logic runs.

### TARGET_CLASS Semantics

When a decorator attribute is placed on a class, the system calls `before()`/`after()` on **every method call** of that class. The decorator itself decides what to do — time each method, track object lifetime, or anything else:

```php
#[Timer]
class PaymentProcessor {
    public function charge() { ... }  // Timer fires
    public function refund() { ... }  // Timer fires
}
```

A lifecycle-style decorator can filter by method name:

```php
public function before(OxPHP\Decorator\Context $ctx): void {
    if ($ctx->method === '__construct') {
        // start tracking object lifetime
    }
}
```

## Execution Order

When multiple decorators are applied, they execute in **attribute order** (top to bottom), with `after()` in reverse:

```php
#[DecoratorA]
#[DecoratorB]
function foo() { ... }
```

```
A.before() → B.before() → foo() → B.after() → A.after()
```

This is stack semantics — the outermost decorator sees the full execution including inner decorators.

## Repeatable Attributes

Decorators can be marked `IS_REPEATABLE`, allowing multiple instances on the same target. Each gets its own cached instance with its own constructor arguments:

```php
#[Attribute(Attribute::TARGET_METHOD | Attribute::IS_REPEATABLE)]
class Notify implements OxPHP\Decorator\AttributeInterface { ... }

class OrderService {
    #[Notify(channel: 'slack')]
    #[Notify(channel: 'email')]
    public function placeOrder(): void { ... }
}
```

## Exception Handling

| Scenario | Behavior |
|----------|----------|
| `before()` throws | Function does NOT execute. Previously-succeeded decorators get `after()` in reverse order (cleanup). |
| Function throws | All decorators' `after()` IS called. `$ctx->hasResult()` returns `false`. |
| `after()` throws | Propagated to the caller. Remaining decorators' `after()` are skipped. |

## Rust Plugin API

Plugins can register decorators in Rust using the `Decorator` trait. These are more efficient than PHP decorators — no PHP object creation or method dispatch overhead.

```rust
use oxphp::decorator::{Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult, AttributeTargets};

struct TimerDecorator;

impl Decorator for TimerDecorator {
    fn attribute_name(&self) -> &str { "App\\Profiler\\Timer" }
    fn targets(&self) -> AttributeTargets { AttributeTargets::ALL }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        // start timing
        DecoratorAction::Continue
    }

    fn on_end(&self, ctx: &DecoratorCallContext, result: &DecoratorCallResult) {
        // record elapsed time
    }
}
```

Register during plugin initialization:

```rust
fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_decorator(TimerDecorator);
    Ok(())
}
```

Both Rust and PHP decorators feed into the same `DecoratorRegistry` and coexist on the same functions.

## Performance

The decorator system is designed for minimal overhead:

- **Zero cost for undecorated functions** — the observer init returns `{NULL, NULL}` for functions without registered decorator attributes. PHP caches this result per op_array, so subsequent calls skip the check entirely.
- **One-time resolution** — attribute-to-decorator mapping happens once per function (on first call), not on every invocation.
- **Arc\<str\> string reuse** — target/class/method strings are allocated once during resolution and shared across all calls via reference counting.

## See Also

- [PHP Functions](../php/functions.md) — `oxphp_register_decorator()` reference
- [Event System](../architecture/event-system.md) — event dispatch (decorators work at the function level, not the request level)
- [SAPI and Bridge](../architecture/sapi-bridge.md) — the C bridge that connects PHP to Rust
