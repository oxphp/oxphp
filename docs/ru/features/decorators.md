---
title: Атрибутные декораторы
description: Перехват вызовов PHP-функций и методов с помощью атрибутов PHP 8+
---

OxPHP предоставляет систему декораторов на основе атрибутов, которая перехватывает вызовы PHP-функций и методов на уровне движка. Декораторы используют PHP 8+ Observer API (`zend_observer_fcall`) для перехвата без накладных расходов для недекорированных функций и прозрачного оборачивания декорированных.

Система предоставляет только **механизм перехвата**. За то, что декораторы делают (измерение времени, метрики, circuit breaking, кеширование), отвечает реализация декоратора.

## Как это работает

1. PHP-класс реализует `OxPHP\Decorator\AttributeInterface` и помечается `#[Attribute]`
2. Класс регистрируется в OxPHP через `oxphp_register_decorator()`
3. Когда атрибут размещается на функции, методе или классе, OxPHP перехватывает каждый вызов
4. Методы `before()` и `after()` декоратора вызываются вокруг исходной функции

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

// Регистрируется один раз при инициализации приложения
oxphp_register_decorator(Timer::class);
```

После регистрации атрибут можно использовать на любой функции или методе:

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

Интерфейс, который должны реализовывать все классы декораторов:

```php
namespace OxPHP\Decorator;

interface AttributeInterface {
    public function before(Context $ctx): void;
    public function after(Context $ctx): void;
}
```

### `OxPHP\Decorator\Context`

Объект контекста (только для чтения), передаваемый в `before()` и `after()`:

| Свойство | Тип | Описание |
|----------|-----|----------|
| `$target` | `string` | Полное имя цели (`App\Service::method` или `my_function`) |
| `$class` | `string` | Имя класса или `""` для функций |
| `$method` | `string` | Имя метода или `""` для функций |
| `$function` | `string` | Имя функции для `TARGET_FUNCTION` или `""` для методов |
| `$objectId` | `int` | `spl_object_id` для методов, `0` для функций |
| `$requestId` | `string` | Идентификатор текущего запроса |
| `$traceId` | `string` | Идентификатор трейса W3C (если трассировка включена) |

| Метод | Возвращает | Описание |
|-------|-----------|----------|
| `getParams()` | `array` | Аргументы, переданные в декорированную функцию (ленивые, без накладных расходов, если не вызывается) |
| `getResult()` | `mixed` | Возвращаемое значение декорированной функции (только в `after()`, возвращает `null` в `before()`) |
| `hasResult()` | `bool` | `true` в `after()`, когда функция вернула значение успешно, `false` иначе |

### `OxPHP\Decorator\RejectedException`

Исключение, выбрасываемое, когда Rust-нативный декоратор отклоняет вызов через `DecoratorAction::Reject`. Расширяет `\Exception`.

### `oxphp_register_decorator()`

```php
oxphp_register_decorator(string $class): bool
```

Регистрирует PHP-класс как декоратор. Класс должен реализовывать `OxPHP\Decorator\AttributeInterface` и быть помечен `#[Attribute(...)]`. Возвращает `true` при успехе, `false` с `E_WARNING` при ошибке валидации.

## Цели атрибутов

Атрибуты декораторов не обязаны поддерживать все цели. Каждый класс декоратора объявляет свои цели через PHP `#[Attribute(...)]`:

```php
// Только методы
#[Attribute(Attribute::TARGET_METHOD)]
class RequireAuth implements OxPHP\Decorator\AttributeInterface { ... }

// Только классы — before()/after() срабатывает для каждого метода класса
#[Attribute(Attribute::TARGET_CLASS)]
class Audited implements OxPHP\Decorator\AttributeInterface { ... }

// Все три
#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements OxPHP\Decorator\AttributeInterface { ... }
```

PHP проверяет цели во время компиляции. Размещение атрибута там, где его флаги цели не разрешают, вызывает ошибку PHP до выполнения какой-либо логики декоратора.

### Семантика TARGET_CLASS

Когда атрибут декоратора размещается на классе, система вызывает `before()`/`after()` для **каждого вызова метода** этого класса. Сам декоратор решает, что делать — измерять время каждого метода, отслеживать время жизни объекта или что-то ещё:

```php
#[Timer]
class PaymentProcessor {
    public function charge() { ... }  // Timer срабатывает
    public function refund() { ... }  // Timer срабатывает
}
```

Декоратор в стиле жизненного цикла может фильтровать по имени метода:

```php
public function before(OxPHP\Decorator\Context $ctx): void {
    if ($ctx->method === '__construct') {
        // начать отслеживание времени жизни объекта
    }
}
```

## Порядок выполнения

При применении нескольких декораторов они выполняются в **порядке атрибутов** (сверху вниз), а `after()` — в обратном порядке:

```php
#[DecoratorA]
#[DecoratorB]
function foo() { ... }
```

```
A.before() → B.before() → foo() → B.after() → A.after()
```

Это стековая семантика — самый внешний декоратор видит полное выполнение, включая внутренние декораторы.

## Повторяемые атрибуты

Декораторы могут быть помечены `IS_REPEATABLE`, что позволяет разместить несколько экземпляров на одной цели. Каждый получает собственный кешированный экземпляр со своими аргументами конструктора:

```php
#[Attribute(Attribute::TARGET_METHOD | Attribute::IS_REPEATABLE)]
class Notify implements OxPHP\Decorator\AttributeInterface { ... }

class OrderService {
    #[Notify(channel: 'slack')]
    #[Notify(channel: 'email')]
    public function placeOrder(): void { ... }
}
```

## Обработка исключений

| Сценарий | Поведение |
|----------|-----------|
| `before()` выбрасывает исключение | Функция НЕ выполняется. Ранее успешно выполненные декораторы получают `after()` в обратном порядке (очистка). |
| Функция выбрасывает исключение | `after()` всех декораторов ВЫЗЫВАЕТСЯ. `$ctx->hasResult()` возвращает `false`. |
| `after()` выбрасывает исключение | Распространяется до вызывающей стороны. `after()` оставшихся декораторов пропускается. |

## Rust Plugin API

Плагины могут регистрировать декораторы в Rust с помощью трейта `Decorator`. Они эффективнее PHP-декораторов — нет накладных расходов на создание PHP-объектов или диспетчеризацию методов.

```rust
use oxphp::decorator::{Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult, AttributeTargets};

struct TimerDecorator;

impl Decorator for TimerDecorator {
    fn attribute_name(&self) -> &str { "App\\Profiler\\Timer" }
    fn targets(&self) -> AttributeTargets { AttributeTargets::ALL }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        // начать измерение времени
        DecoratorAction::Continue
    }

    fn on_end(&self, ctx: &DecoratorCallContext, result: &DecoratorCallResult) {
        // записать прошедшее время
    }
}
```

Регистрация во время инициализации плагина:

```rust
fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_decorator(TimerDecorator);
    Ok(())
}
```

Декораторы Rust и PHP используют один `DecoratorRegistry` и могут сосуществовать на одних и тех же функциях.

## Производительность

Система декораторов разработана с минимальными накладными расходами:

- **Нулевая стоимость для недекорированных функций** — инициализация observer возвращает `{NULL, NULL}` для функций без зарегистрированных атрибутов декораторов. PHP кеширует этот результат для каждого op_array, так что последующие вызовы полностью пропускают проверку.
- **Однократное разрешение** — сопоставление атрибут-декоратор происходит один раз для каждой функции (при первом вызове), а не при каждом вызове.
- **Кеширование экземпляров** — PHP-объекты декораторов создаются один раз для каждой пары функция-декоратор (с аргументами конструктора, прочитанными из атрибута), затем кешируются в потоко-локальном TLS на время запроса (или воркера). При последующих вызовах объекты не создаются.
- **Повторное использование строк `Arc<str>`** — строки target/class/method выделяются один раз во время разрешения и разделяются между всеми вызовами через подсчёт ссылок.
- **Декораторы Rust полностью обходят накладные расходы PHP** — `on_begin()`/`on_end()` вызываются напрямую через FFI без создания PHP-объектов, диспетчеризации методов или манипуляций с zval.

## Смотрите также

- [Функции PHP](../php/functions.md) — справочник `oxphp_register_decorator()`
- [Система событий](../architecture/event-system.md) — диспетчеризация событий (декораторы работают на уровне функций, а не на уровне запросов)
- [SAPI и мост](../architecture/sapi-bridge.md) — C-мост, соединяющий PHP и Rust
