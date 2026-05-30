---
title: 装饰器
description: 使用基于属性的装饰器拦截 PHP 函数和方法调用，实现日志记录、计时、缓存和访问控制。
---

# 装饰器

OxPHP 装饰器使用 PHP 8 属性拦截 PHP 函数和方法调用。为任意函数或方法添加属性后，OxPHP 会在每次调用前后分别执行装饰器的 `before()` 和 `after()` 方法——无需修改原始代码。

## 工作原理

1. **定义** 一个实现 `OxPHP\Decorator\AttributeInterface` 并使用 `#[Attribute]` 注解的装饰器类
2. **注册** 在启动时通过 `oxphp_register_decorator(ClassName::class)` 注册一次
3. **应用** 将属性添加到任意函数、方法或类上
4. 首次调用被装饰的函数时，OxPHP 检测到该属性并安装拦截钩子
5. 后续每次调用时，`before()` 在函数执行前运行，`after()` 在函数返回后运行

## 编写装饰器

装饰器类需要两个要素：`#[Attribute]` 注解和 `AttributeInterface` 实现。

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

在启动时注册装饰器，在任何被装饰函数调用之前完成注册：

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

oxphp_register_decorator(Timer::class);
```

将其应用于函数和方法：

```php
<?php

#[Timer]
function processOrder(int $orderId): void
{
    // Timer::before() 在此之前运行
    // Timer::after() 在此之后运行
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

### 类级装饰器

将属性应用于类，以装饰其所有方法：

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

// 注册
oxphp_register_decorator(Audit::class);

// 此类中的所有方法现在都会被审计
#[Audit]
class OrderService
{
    public function create(array $data): int { /* ... */ }
    public function cancel(int $id): void { /* ... */ }
}
```

## Context 对象

`before()` 和 `after()` 都会接收一个 `OxPHP\Decorator\Context` 对象，其中包含被装饰调用的相关信息。

### 属性

| 属性 | 类型 | 说明 |
|----------|------|-------------|
| `$target` | `string` | 完整目标名称：`App\Service::method` 或 `my_function` |
| `$class` | `string` | 类名，独立函数为 `""` |
| `$method` | `string` | 方法名，独立函数为 `""` |
| `$function` | `string` | 独立函数的函数名，方法为 `""` |
| `$objectId` | `int` | 被调用对象的 `spl_object_id()`，函数和静态方法为 `0` |
| `$requestId` | `string` | 当前请求 ID |
| `$traceId` | `string` | 当前 W3C trace ID。未启用分布式追踪时为空字符串 |

### 方法

| 方法 | 可用于 | 说明 |
|--------|-------------|-------------|
| `getParams(): array` | `before()` 和 `after()` | 传递给被装饰函数的参数 |
| `getResult(): mixed` | 仅 `after()` | 被装饰函数的返回值。在 `before()` 中或发生异常后返回 `null` |
| `hasResult(): bool` | 仅 `after()` | 函数成功返回（未抛出异常）时为 `true` |

### 检查参数

`getParams()` 以数字索引数组的形式返回参数：

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

## 多个装饰器

可以在同一个函数上叠加多个装饰器。`before()` 按声明顺序执行，`after()` 按反向顺序执行：

```php
<?php

#[RateLimit(maxCalls: 100, windowSeconds: 60)]
#[Timer]
#[Cache(ttl: 300)]
function getProduct(int $id): array
{
    // 执行顺序：
    // 1. RateLimit::before()
    // 2. Timer::before()
    // 3. Cache::before()
    // 4. getProduct() 执行
    // 5. Cache::after()
    // 6. Timer::after()
    // 7. RateLimit::after()
}
```

如果 `before()` 抛出异常，OxPHP 会记录该异常，并对所有已成功完成 `before()` 的装饰器按反向顺序调用 `after()`。调用方可以通过普通的 PHP `try`/`catch` 捕获该异常，而对于 `before()` 抛出异常的那个装饰器本身，**不会**再调用其 `after()`。

> **关于"阻止执行"的重要说明。** OxPHP 使用 PHP 的 `zend_observer_fcall_begin` API 来调用 `before()`，而该 API 并未提供取消函数调用本身的能力。当 `before()` 抛出异常时，被装饰函数的函数体在 VM 展开到最近的异常处理器之前，仍可能执行若干条 opcode。**请不要依赖 `RejectedException` 来跳过函数体内部的副作用。** 应将装饰器的拒绝理解为"调用方会看到一个异常",而真正的硬性权限校验应放在函数体内部(或调用之前),而不是放在装饰器里。

## 阻止执行

装饰器可以通过在 `before()` 中抛出 `OxPHP\Decorator\RejectedException` 来表示拒绝。异常会传播到调用方,但(如上所述)这并不是"调用前否决":

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

## Worker 模式行为

在 Worker 模式下，装饰器实例在同一 Worker 的多个请求间持久化——它们只创建一次并被复用。这意味着：

- 构造函数逻辑每个 Worker 每个被装饰函数只运行一次（而非每个请求）
- 属性中的实例状态会在请求间延续
- Worker 被回收时，实例会重新创建

将装饰器设计为在请求间无状态。如果需要请求级状态，在 `before()` 中设置，在 `after()` 中读取：

```php
<?php

#[Attribute(Attribute::TARGET_METHOD)]
class RequestTimer implements AttributeInterface
{
    // 请求级状态：在 before() 中设置，在 after() 中读取
    private float $start;

    public function before(Context $ctx): void
    {
        $this->start = hrtime(true);
    }

    public function after(Context $ctx): void
    {
        $elapsed = (hrtime(true) - $this->start) / 1e6;
        // 安全：$this->start 始终在 before() 中新鲜设置
    }
}
```

## 内置装饰器

### #[OxPHP\Apm\Trace]

当 APM 插件启用时（`OTEL_APM_ENABLED=true`），OxPHP 会为 `#[OxPHP\Apm\Trace]` 属性注册一个内置装饰器。它在函数进入时自动创建 Span，在函数退出时自动关闭——无需手动调用 `oxphp_apm_start()` / `oxphp_apm_end()`。

```php
<?php
use OxPHP\Apm\Trace;

#[Trace]
function processOrder(int $orderId): void
{
    // 名为 "processOrder" 的 Span 会自动创建。
    // 如果此函数抛出异常，Span 会被标记为错误
    // 并记录一个包含类名的 "exception" 事件。
}

class PaymentService
{
    #[Trace]
    public function charge(float $amount): bool
    {
        // Span 名称为 "PaymentService::charge"
        return true;
    }
}
```

`#[Trace]` 属性同时支持函数和方法。它适用于用户定义的 PHP 代码（不适用于内部 C 函数——那些由 APM 自动埋点钩子处理）。

无需调用 `oxphp_register_decorator()`——APM 插件在服务器初始化期间会自动注册此装饰器。该装饰器在标准模式和 Worker 模式下均可使用。

关于 APM 追踪的更多信息，请参阅[分布式追踪与 APM](distributed-tracing.md)。

## 限制

- **仅限用户定义函数** — PHP 内置函数无法被装饰。只有 PHP 代码中定义的函数和方法可被拦截
- **首次调用前完成注册** — 装饰器必须在目标函数首次调用之前注册完成。请在启动时注册
- **标量构造函数参数** — 属性构造函数参数在首次调用时评估一次。属性中的复杂表达式或运行时值不受支持
- **最大 256 层嵌套** — 装饰器上下文栈最多支持 256 层嵌套的被装饰函数调用。超过此限制时，调用会抛出 `OxPHP\Decorator\StackOverflowException`，而不是静默破坏装饰器上下文

## 故障排除

### 装饰器未拦截调用

装饰器在函数首次调用之后才注册，或属性未被识别。

**检查：** 确保在任何被装饰函数调用之前先调用 `oxphp_register_decorator()`。在 Worker 模式下，请在 `oxphp_worker()` 之前的外层作用域中注册。

### 注册时提示"Class not found"

调用 `oxphp_register_decorator()` 时，装饰器类尚未加载。

**修复：** 确保先注册自动加载器：

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

// 现在可以找到该类
oxphp_register_decorator(Timer::class);
```

### 构造函数参数在请求间未更新

装饰器实例按 Worker 缓存，构造函数只运行一次，而非每个请求。

**修复：** 使用 `before()` 进行请求级初始化，而非构造函数。构造函数只应接受属性中的静态配置。

## PHP 示例

### 缓存装饰器

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
            // 跳过函数执行——返回缓存值
            // 注意：无法从 PHP 装饰器中短路执行。
            // 请在函数本身中配合外部缓存检查使用此模式。
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

### 带请求上下文的日志装饰器

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

## 参见

- [PHP 函数](../php/functions.md) -- `oxphp_register_decorator()` 参考
- [Worker 模式](worker-mode.md) -- 持久化 Worker 如何影响装饰器实例的生命周期
- [分布式追踪](distributed-tracing.md) -- 结合 trace context 使用装饰器创建自定义 span
