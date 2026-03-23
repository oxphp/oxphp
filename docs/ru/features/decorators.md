---
title: Декораторы
description: Перехватывайте вызовы PHP-функций и методов с помощью декораторов на основе атрибутов для логирования, измерения времени, кэширования и управления доступом.
---

# Декораторы

Декораторы OxPHP перехватывают вызовы PHP-функций и методов с использованием атрибутов PHP 8. Добавьте атрибут к любой функции или методу, и OxPHP вызовет методы `before()` и `after()` вашего декоратора вокруг каждого вызова — без единого изменения исходного кода.

## Как это работает

1. **Определите** класс декоратора, реализующий `OxPHP\Decorator\AttributeInterface` и аннотированный `#[Attribute]`
2. **Зарегистрируйте** его один раз при инициализации через `oxphp_register_decorator(ClassName::class)`
3. **Примените** атрибут к любой функции, методу или классу
4. При первом вызове декорированной функции OxPHP обнаруживает атрибут и устанавливает хуки перехвата
5. При каждом последующем вызове `before()` выполняется перед функцией, а `after()` — после её возврата

## Написание декоратора

Классу декоратора нужны две вещи: аннотация `#[Attribute]` и реализация `AttributeInterface`.

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

Зарегистрируйте декоратор во время инициализации, до вызова любой декорированной функции:

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

oxphp_register_decorator(Timer::class);
```

Примените его к функциям и методам:

```php
<?php

#[Timer]
function processOrder(int $orderId): void
{
    // Timer::before() выполняется до этого
    // Timer::after() выполняется после этого
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

### Декораторы уровня класса

Примените атрибут к классу, чтобы декорировать все его методы:

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

// Регистрация
oxphp_register_decorator(Audit::class);

// Каждый метод этого класса теперь аудируется
#[Audit]
class OrderService
{
    public function create(array $data): int { /* ... */ }
    public function cancel(int $id): void { /* ... */ }
}
```

## Объект Context

Оба метода — `before()` и `after()` — получают объект `OxPHP\Decorator\Context` с информацией о декорированном вызове.

### Свойства

| Свойство | Тип | Описание |
|----------|-----|----------|
| `$target` | `string` | Полное имя цели: `App\Service::method` или `my_function` |
| `$class` | `string` | Имя класса, или `""` для обычных функций |
| `$method` | `string` | Имя метода, или `""` для обычных функций |
| `$function` | `string` | Имя функции для обычных функций, или `""` для методов |
| `$objectId` | `int` | `spl_object_id()` вызываемого объекта, `0` для функций и статических методов |
| `$requestId` | `string` | Идентификатор текущего запроса |

### Методы

| Метод | Доступен в | Описание |
|-------|-----------|----------|
| `getParams(): array` | `before()` и `after()` | Аргументы, переданные в декорированную функцию |
| `getResult(): mixed` | только `after()` | Возвращаемое значение декорированной функции. Возвращает `null` в `before()` или после исключения |
| `hasResult(): bool` | только `after()` | `true`, если функция вернула результат без выброса исключения |

### Проверка аргументов

`getParams()` возвращает аргументы в виде массива с числовыми индексами:

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

## Несколько декораторов

Вы можете накапливать несколько декораторов на одной функции. Они выполняются в порядке объявления для `before()` и в обратном порядке для `after()`:

```php
<?php

#[RateLimit(maxCalls: 100, windowSeconds: 60)]
#[Timer]
#[Cache(ttl: 300)]
function getProduct(int $id): array
{
    // Порядок выполнения:
    // 1. RateLimit::before()
    // 2. Timer::before()
    // 3. Cache::before()
    // 4. getProduct() выполняется
    // 5. Cache::after()
    // 6. Timer::after()
    // 7. RateLimit::after()
}
```

Если `before()` выбрасывает исключение, функция не выполняется, а `after()` вызывается в обратном порядке для всех декораторов, которые уже завершили свой `before()`.

## Остановка выполнения

Декоратор может предотвратить выполнение функции, выбросив `OxPHP\Decorator\RejectedException` из `before()`:

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

## Поведение в режиме воркера

В режиме воркера экземпляры декораторов сохраняются между запросами в рамках одного воркера — они создаются один раз и переиспользуются. Это означает:

- Логика конструктора выполняется один раз на воркер для каждой декорированной функции (не на каждый запрос)
- Состояние экземпляра в свойствах переносится между запросами
- Экземпляры пересоздаются при перезапуске воркера

Проектируйте декораторы так, чтобы они не хранили состояние между запросами. Если вам нужно состояние для конкретного запроса, устанавливайте его в `before()` и читайте в `after()`:

```php
<?php

#[Attribute(Attribute::TARGET_METHOD)]
class RequestTimer implements AttributeInterface
{
    // Состояние для запроса: устанавливается в before(), читается в after()
    private float $start;

    public function before(Context $ctx): void
    {
        $this->start = hrtime(true);
    }

    public function after(Context $ctx): void
    {
        $elapsed = (hrtime(true) - $this->start) / 1e6;
        // Безопасно: $this->start всегда задаётся заново в before()
    }
}
```

## Ограничения

- **Только пользовательские функции** — встроенные функции PHP не могут быть декорированы. Перехватить можно только функции и методы, определённые в PHP-коде
- **Регистрация до первого вызова** — декораторы должны быть зарегистрированы до первого вызова любой целевой функции. Регистрируйте во время инициализации
- **Скалярные аргументы конструктора** — аргументы конструктора атрибута вычисляются один раз при первом вызове. Сложные выражения или значения времени выполнения в атрибутах не поддерживаются
- **Максимум 32 уровня вложенности** — стек контекста декоратора поддерживает до 32 уровней вложенных вызовов декорированных функций
- **Максимум 256 кэшированных экземпляров на воркер** — экземпляры декораторов кэшируются в потоке воркера. Коллизии возможны в приложениях с более чем 256 уникальными парами «декоратор — функция»

## Устранение неполадок

### Декоратор не перехватывает вызовы

Декоратор зарегистрирован после того, как функция уже была вызвана, или атрибут не распознан.

**Проверьте:** убедитесь, что `oxphp_register_decorator()` вызывается до первого вызова любой декорированной функции. В режиме воркера регистрируйте во внешней области видимости до `oxphp_worker()`.

### "Class not found" при регистрации

Класс декоратора не загружен на момент вызова `oxphp_register_decorator()`.

**Решение:** убедитесь, что автозагрузчик зарегистрирован первым:

```php
<?php
require __DIR__ . '/../vendor/autoload.php';

// Теперь класс можно найти
oxphp_register_decorator(Timer::class);
```

### Аргументы конструктора не обновляются между запросами

Экземпляры декораторов кэшируются на воркер. Конструктор выполняется один раз, а не при каждом запросе.

**Решение:** используйте `before()` для инициализации, специфичной для запроса, а не конструктор. Конструктор должен принимать только статическую конфигурацию из атрибута.

## Примеры PHP

### Декоратор кэширования

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
            // Пропустить выполнение функции — вернуть кэшированное значение
            // Примечание: нельзя прервать выполнение из PHP-декораторов.
            // Используйте этот шаблон вместе с проверкой внешнего кэша в самой функции.
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

### Декоратор логирования с контекстом запроса

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

## См. также

- [PHP Functions](../php/functions.md) — справочник по `oxphp_register_decorator()`
- [Worker Mode](worker-mode.md) — как постоянные воркеры влияют на время жизни экземпляров декораторов
- [Distributed Tracing](distributed-tracing.md) — использование декораторов с контекстом трассировки для пользовательских спанов
