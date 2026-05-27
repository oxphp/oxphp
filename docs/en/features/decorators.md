---
title: Decorators
description: Intercept PHP function and method calls with attribute-based decorators for logging, timing, caching, and access control.
---

# Decorators

OxPHP decorators intercept PHP function and method calls using PHP 8 attributes. Add an attribute to any function or method, and OxPHP calls your decorator's `before()` and `after()` methods around every invocation — with zero changes to the original code.

## How It Works

1. **Define** a decorator class that implements `OxPHP\Decorator\AttributeInterface` and is annotated with `#[Attribute]`
2. **Register** it once at bootstrap with `oxphp_register_decorator(ClassName::class)`
3. **Apply** the attribute to any function, method, or class
4. On the first call to a decorated function, OxPHP detects the attribute and installs interception hooks
5. On every subsequent call, `before()` runs before the function and `after()` runs after it returns

## Writing a Decorator

A decorator class needs two things: the `#[Attribute]` annotation and the `AttributeInterface` implementation.

```php
<?php

use OxPHP\Decorator\AttributeInterface;
use OxPHP\Decorator\Context;

#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements AttributeInterface
{
    private float $start;

    public function __construct(
        public readonly string $label = '',
    ) {}

    public function before(Context $ctx): void
    {
        $this->start = hrtime(true);
    }

    public function after(Context $ctx): void
    {
        $elapsed = (hrtime(true) - $this->start) / 1e6;
        error_log(sprintf('[Timer] %s: %.2fms', $this->label ?: $ctx->target, $elapsed));
    }
}
```

Register the decorator during bootstrap, before any decorated function is called:

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

oxphp_register_decorator(Timer::class);
```

Apply it to functions and methods:

```php
<?php

#[Timer]
function processOrder(int $orderId): void
{
    // Timer::before() runs before this
    // Timer::after() runs after this
}

#[Timer(label: 'db-query')]
function fetchUser(int $id): array
{
    return $db->query('SELECT * FROM users WHERE id = ?', [$id]);
}

class PaymentService
{
    #[Timer(label: 'payment')]
    public function charge(float $amount): bool
    {
        // ...
    }
}
```

### Class-Level Decorators

Apply an attribute to a class to decorate all its methods:

```php
<?php

#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_METHOD)]
class Audit implements AttributeInterface
{
    public function before(Context $ctx): void
    {
        error_log("Calling {$ctx->target}");
    }

    public function after(Context $ctx): void
    {
        $status = $ctx->hasResult() ? 'ok' : 'error';
        error_log("Finished {$ctx->target}: {$status}");
    }
}

// Register
oxphp_register_decorator(Audit::class);

// Every method in this class is now audited
#[Audit]
class OrderService
{
    public function create(array $data): int { /* ... */ }
    public function cancel(int $id): void { /* ... */ }
}
```

## The Context Object

Both `before()` and `after()` receive an `OxPHP\Decorator\Context` object with information about the decorated call.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `$target` | `string` | Full target name: `App\Service::method` or `my_function` |
| `$class` | `string` | Class name, or `""` for standalone functions |
| `$method` | `string` | Method name, or `""` for standalone functions |
| `$function` | `string` | Function name for standalone functions, or `""` for methods |
| `$objectId` | `int` | `spl_object_id()` of the called object, `0` for functions and static methods |
| `$requestId` | `string` | Current request ID |
| `$traceId` | `string` | Current W3C trace ID. Empty string when distributed tracing is not active |

### Methods

| Method | Available in | Description |
|--------|-------------|-------------|
| `getParams(): array` | `before()` and `after()` | Arguments passed to the decorated function |
| `getResult(): mixed` | `after()` only | Return value of the decorated function. Returns `null` in `before()` or after an exception |
| `hasResult(): bool` | `after()` only | `true` if the function returned successfully without throwing |

### Inspecting Arguments

`getParams()` returns the arguments as a numerically-indexed array:

```php
<?php

#[Attribute(Attribute::TARGET_FUNCTION)]
class ValidateArgs implements AttributeInterface
{
    public function before(Context $ctx): void
    {
        $params = $ctx->getParams();
        foreach ($params as $i => $value) {
            if ($value === null) {
                throw new \InvalidArgumentException(
                    "Argument {$i} of {$ctx->target} must not be null"
                );
            }
        }
    }

    public function after(Context $ctx): void {}
}
```

## Multiple Decorators

Stack multiple decorators on the same function. They execute in declaration order for `before()` and reverse order for `after()`:

```php
<?php

#[RateLimit(maxCalls: 100, windowSeconds: 60)]
#[Timer]
#[Cache(ttl: 300)]
function getProduct(int $id): array
{
    // Execution order:
    // 1. RateLimit::before()
    // 2. Timer::before()
    // 3. Cache::before()
    // 4. getProduct() executes
    // 5. Cache::after()
    // 6. Timer::after()
    // 7. RateLimit::after()
}
```

If `before()` throws an exception, OxPHP records the exception and invokes `after()` in reverse order on all decorators that already completed their `before()`. The caller will observe the exception via normal PHP `try`/`catch`, and `after()` is **not** called on the decorator whose `before()` threw.

> **Important caveat about stopping execution.** PHP's `zend_observer_fcall_begin` API — which OxPHP uses to invoke `before()` — does not expose a way to cancel the call itself. When `before()` throws, the decorated function body may still execute a handful of opcodes before the VM unwinds to the nearest exception handler. **Do not rely on `RejectedException` to skip side effects inside the function body.** Treat decorator rejection as "the caller sees an exception" and implement any hard authorization gate inside the function body (or in front of it), not in the decorator.

## Stopping Execution

A decorator can signal rejection by throwing `OxPHP\Decorator\RejectedException` from `before()`. The exception propagates to the caller, but (as noted above) it is not a pre-call veto:

```php
<?php

#[Attribute(Attribute::TARGET_METHOD)]
class RequireRole implements AttributeInterface
{
    public function __construct(
        public readonly string $role,
    ) {}

    public function before(Context $ctx): void
    {
        if (!current_user_has_role($this->role)) {
            throw new \OxPHP\Decorator\RejectedException(
                "Access denied: requires role '{$this->role}'"
            );
        }
    }

    public function after(Context $ctx): void {}
}
```

## Worker Mode Behavior

In worker mode, decorator instances persist across requests within the same worker — they are created once and reused. This means:

- Constructor logic runs once per worker per decorated function (not per request)
- Instance state in properties carries over between requests
- Instances are recreated when the worker is recycled

Design decorators to be stateless between requests. If you need per-request state, set it in `before()` and read it in `after()`:

```php
<?php

#[Attribute(Attribute::TARGET_METHOD)]
class RequestTimer implements AttributeInterface
{
    // Per-request state: set in before(), read in after()
    private float $start;

    public function before(Context $ctx): void
    {
        $this->start = hrtime(true);
    }

    public function after(Context $ctx): void
    {
        $elapsed = (hrtime(true) - $this->start) / 1e6;
        // Safe: $this->start is always set fresh in before()
    }
}
```

## Built-in Decorators

### #[OxPHP\Apm\Trace]

When the APM plugin is enabled (`OTEL_APM_ENABLED=true`), OxPHP registers a built-in decorator for the `#[OxPHP\Apm\Trace]` attribute. It automatically creates a span on function entry and closes it on exit — no manual `oxphp_apm_start()` / `oxphp_apm_end()` calls needed.

```php
<?php
use OxPHP\Apm\Trace;

#[Trace]
function processOrder(int $orderId): void
{
    // A span named "processOrder" is created automatically.
    // If this function throws, the span is marked as error
    // and an "exception" event is recorded with the class name.
}

class PaymentService
{
    #[Trace]
    public function charge(float $amount): bool
    {
        // Span named "PaymentService::charge"
        return true;
    }
}
```

The `#[Trace]` attribute targets both functions and methods. It works on user-defined PHP code (not internal C functions — those are handled by the APM auto-instrumentation hooks).

No `oxphp_register_decorator()` call is needed — the APM plugin registers this decorator automatically during server initialization. The decorator is available in both standard and worker modes.

For more on APM tracing, see [Distributed Tracing & APM](distributed-tracing.md).

## Limitations

- **User functions only** — built-in PHP functions cannot be decorated. Only functions and methods defined in PHP code are interceptable
- **Registration before first call** — decorators must be registered before the first invocation of any function they target. Register during bootstrap
- **Scalar constructor arguments** — attribute constructor arguments are evaluated once at first call. Complex expressions or runtime values in attributes are not supported
- **Max 32 nesting levels** — the decorator context stack supports up to 32 levels of nested decorated function calls

## Troubleshooting

### Decorator not intercepting calls

The decorator is registered after the function was already called, or the attribute is not recognized.

**Check:** Ensure `oxphp_register_decorator()` is called before any decorated function is invoked. In worker mode, register in the outer scope before `oxphp_worker()`.

### "Class not found" when registering

The decorator class is not loaded when `oxphp_register_decorator()` is called.

**Fix:** Ensure the autoloader is registered first:

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

// Now the class can be found
oxphp_register_decorator(Timer::class);
```

### Constructor arguments not updating between requests

Decorator instances are cached per worker. The constructor runs once, not per request.

**Fix:** Use `before()` for per-request initialization, not the constructor. The constructor should only accept static configuration from the attribute.

## PHP Examples

### Caching Decorator

```php
<?php

#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Cache implements AttributeInterface
{
    private static array $store = [];

    public function __construct(
        public readonly int $ttl = 60,
    ) {}

    public function before(Context $ctx): void
    {
        $key = $ctx->target . ':' . serialize($ctx->getParams());
        if (isset(self::$store[$key]) && self::$store[$key]['expires'] > time()) {
            // Skip function execution — return cached value
            // Note: you cannot short-circuit execution from PHP decorators.
            // Use this pattern with an external cache check in the function itself.
        }
    }

    public function after(Context $ctx): void
    {
        if ($ctx->hasResult()) {
            $key = $ctx->target . ':' . serialize($ctx->getParams());
            self::$store[$key] = [
                'value' => $ctx->getResult(),
                'expires' => time() + $this->ttl,
            ];
        }
    }
}
```

### Logging Decorator with Request Context

```php
<?php

#[Attribute(Attribute::TARGET_METHOD)]
class LogCall implements AttributeInterface
{
    public function before(Context $ctx): void
    {
        error_log(json_encode([
            'event' => 'call_start',
            'target' => $ctx->target,
            'request_id' => $ctx->requestId,
            'params' => $ctx->getParams(),
        ]));
    }

    public function after(Context $ctx): void
    {
        error_log(json_encode([
            'event' => 'call_end',
            'target' => $ctx->target,
            'request_id' => $ctx->requestId,
            'success' => $ctx->hasResult(),
        ]));
    }
}
```

## See Also

- [PHP Functions](../php/functions.md) -- `oxphp_register_decorator()` reference
- [Worker Mode](worker-mode.md) -- how persistent workers affect decorator instance lifetime
- [Distributed Tracing](distributed-tracing.md) -- using decorators with trace context for custom spans
