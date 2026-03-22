---
title: 基于属性的装饰器
description: 使用 PHP 8+ 属性拦截 PHP 函数和方法调用
---

OxPHP 提供了一套基于属性的装饰器系统，在引擎层面拦截 PHP 函数和方法调用。装饰器使用 PHP 8+ Observer API（`zend_observer_fcall`）实现对未装饰函数的零开销拦截，以及对已装饰函数的透明包装。

该系统仅提供**拦截机制**。装饰器的具体行为（计时、指标、熔断、缓存）由装饰器实现本身负责。

## 工作原理

1. PHP 类实现 `OxPHP\Decorator\AttributeInterface` 并标记 `#[Attribute]`
2. 通过 `oxphp_register_decorator()` 将该类注册到 OxPHP
3. 当属性应用于函数、方法或类时，OxPHP 拦截每次调用
4. 装饰器的 `before()` 和 `after()` 方法在原始函数前后触发

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

// 在应用启动时注册一次
oxphp_register_decorator(Timer::class);
```

注册后，可在任意函数或方法上使用该属性：

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

所有装饰器类必须实现的接口：

```php
namespace OxPHP\Decorator;

interface AttributeInterface {
    public function before(Context $ctx): void;
    public function after(Context $ctx): void;
}
```

### `OxPHP\Decorator\Context`

传递给 `before()` 和 `after()` 的只读上下文对象：

| 属性 | 类型 | 说明 |
|------|------|------|
| `$target` | `string` | 完整目标名称（`App\Service::method` 或 `my_function`） |
| `$class` | `string` | 类名，函数则为 `""` |
| `$method` | `string` | 方法名，函数则为 `""` |
| `$function` | `string` | `TARGET_FUNCTION` 的函数名，方法则为 `""` |
| `$objectId` | `int` | 方法的 `spl_object_id`，函数则为 `0` |
| `$requestId` | `string` | 当前请求 ID |
| `$traceId` | `string` | W3C trace ID（如果启用了追踪） |

| 方法 | 返回值 | 说明 |
|------|--------|------|
| `getParams()` | `array` | 传递给被装饰函数的参数（惰性求值，不调用则零开销） |
| `getResult()` | `mixed` | 被装饰函数的返回值（仅在 `after()` 中有效，`before()` 中返回 `null`） |
| `hasResult()` | `bool` | 函数成功返回时 `after()` 中为 `true`，否则为 `false` |

### `OxPHP\Decorator\RejectedException`

当 Rust 原生装饰器通过 `DecoratorAction::Reject` 拒绝调用时抛出的异常。继承自 `\Exception`。

### `oxphp_register_decorator()`

```php
oxphp_register_decorator(string $class): bool
```

将 PHP 类注册为装饰器。该类必须实现 `OxPHP\Decorator\AttributeInterface` 并标记 `#[Attribute(...)]`。成功返回 `true`，验证失败时触发 `E_WARNING` 并返回 `false`。

## 属性目标

装饰器属性不要求支持所有目标。每个装饰器类通过 PHP 的 `#[Attribute(...)]` 声明自己支持的目标：

```php
// 仅方法
#[Attribute(Attribute::TARGET_METHOD)]
class RequireAuth implements OxPHP\Decorator\AttributeInterface { ... }

// 仅类 — 对类的每个方法触发 before()/after()
#[Attribute(Attribute::TARGET_CLASS)]
class Audited implements OxPHP\Decorator\AttributeInterface { ... }

// 全部三种目标
#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements OxPHP\Decorator\AttributeInterface { ... }
```

PHP 在编译时验证目标。将属性放置在其目标标志不允许的位置会在任何装饰器逻辑运行之前产生 PHP 错误。

### TARGET_CLASS 语义

当装饰器属性应用于类时，系统对该类的**每次方法调用**都会调用 `before()`/`after()`。装饰器自行决定行为——对每个方法计时、追踪对象生命周期或其他任何操作：

```php
#[Timer]
class PaymentProcessor {
    public function charge() { ... }  // Timer 触发
    public function refund() { ... }  // Timer 触发
}
```

生命周期风格的装饰器可以按方法名过滤：

```php
public function before(OxPHP\Decorator\Context $ctx): void {
    if ($ctx->method === '__construct') {
        // 开始追踪对象生命周期
    }
}
```

## 执行顺序

当应用多个装饰器时，按**属性顺序**执行（从上到下），`after()` 则以逆序执行：

```php
#[DecoratorA]
#[DecoratorB]
function foo() { ... }
```

```
A.before() → B.before() → foo() → B.after() → A.after()
```

这是栈语义——最外层的装饰器能看到包含内层装饰器在内的完整执行过程。

## 可重复属性

装饰器可以标记为 `IS_REPEATABLE`，允许在同一目标上使用多个实例。每个实例都有自己缓存的对象及其构造参数：

```php
#[Attribute(Attribute::TARGET_METHOD | Attribute::IS_REPEATABLE)]
class Notify implements OxPHP\Decorator\AttributeInterface { ... }

class OrderService {
    #[Notify(channel: 'slack')]
    #[Notify(channel: 'email')]
    public function placeOrder(): void { ... }
}
```

## 异常处理

| 场景 | 行为 |
|------|------|
| `before()` 抛出异常 | 函数**不**执行。已成功执行的装饰器按逆序调用 `after()`（清理）。 |
| 函数抛出异常 | 所有装饰器的 `after()` **都会**被调用。`$ctx->hasResult()` 返回 `false`。 |
| `after()` 抛出异常 | 异常传播给调用者。其余装饰器的 `after()` 被跳过。 |

## Rust 插件 API

插件可以使用 `Decorator` trait 在 Rust 中注册装饰器。这比 PHP 装饰器更高效——没有 PHP 对象创建或方法分发开销。

```rust
use oxphp::decorator::{Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult, AttributeTargets};

struct TimerDecorator;

impl Decorator for TimerDecorator {
    fn attribute_name(&self) -> &str { "App\\Profiler\\Timer" }
    fn targets(&self) -> AttributeTargets { AttributeTargets::ALL }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        // 开始计时
        DecoratorAction::Continue
    }

    fn on_end(&self, ctx: &DecoratorCallContext, result: &DecoratorCallResult) {
        // 记录经过时间
    }
}
```

在插件初始化期间注册：

```rust
fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_decorator(TimerDecorator);
    Ok(())
}
```

Rust 和 PHP 装饰器都注册到同一个 `DecoratorRegistry`，可以共存于同一函数上。

## 性能

装饰器系统设计为最小开销：

- **未装饰函数零开销** — 对于没有注册装饰器属性的函数，observer init 返回 `{NULL, NULL}`。PHP 按 op_array 缓存此结果，后续调用完全跳过检查。
- **一次性解析** — 属性到装饰器的映射每个函数只发生一次（首次调用时），而非每次调用。
- **实例缓存** — PHP 装饰器对象每个函数-装饰器对只实例化一次（从属性中读取构造参数），然后在每线程 TLS 中为请求（或工作进程）生命周期缓存。后续调用不会创建对象。
- **`Arc<str>` 字符串复用** — target/class/method 字符串在解析时分配一次，通过引用计数在所有调用间共享。
- **Rust 装饰器完全跳过 PHP 开销** — `on_begin()`/`on_end()` 通过 FFI 直接调用，没有 PHP 对象创建、方法分发或 zval 操作。

## 另请参阅

- [PHP 函数](../php/functions.md) — `oxphp_register_decorator()` 参考
- [事件系统](../architecture/event-system.md) — 事件分发（装饰器在函数级别工作，而非请求级别）
- [SAPI 与桥接](../architecture/sapi-bridge.md) — 连接 PHP 与 Rust 的 C 桥接库
