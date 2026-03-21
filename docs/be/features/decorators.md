---
title: Дэкаратары на аснове атрыбутаў
description: Перахоп выклікаў PHP-функцый і метадаў праз атрыбуты PHP 8+
---

OxPHP прадастаўляе сістэму дэкаратараў на аснове атрыбутаў, якая перахоплівае выклікі PHP-функцый і метадаў на ўзроўні рухавіка. Дэкаратары выкарыстоўваюць Observer API PHP 8+ (`zend_observer_fcall`) для перахопу недэкараваных функцый з нулявым накладным коштам і празрыстага абгортвання дэкараваных.

Сістэма прадастаўляе толькі **механізм перахопу**. Тое, што робяць дэкаратары (вымярэнне часу, метрыкі, размыканне ланцугу, кэшаванне), — адказнасць рэалізацыі дэкаратара.

## Як гэта працуе

1. PHP-клас рэалізуе `OxPHP\Decorator\AttributeInterface` і пазначаецца `#[Attribute]`
2. Клас рэгіструецца ў OxPHP праз `oxphp_register_decorator()`
3. Калі атрыбут размяшчаецца на функцыі, метадзе або класе, OxPHP перахоплівае кожны выклік
4. Метады `before()` і `after()` дэкаратара выконваюцца вакол зыходнай функцыі

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

// Зарэгіструйце адзін раз пры ініцыялізацыі прыкладання
oxphp_register_decorator(Timer::class);
```

Пасля рэгістрацыі выкарыстоўвайце атрыбут на любой функцыі або метадзе:

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

Інтэрфейс, які павінны рэалізаваць усе класы дэкаратараў:

```php
namespace OxPHP\Decorator;

interface AttributeInterface {
    public function before(Context $ctx): void;
    public function after(Context $ctx): void;
}
```

### `OxPHP\Decorator\Context`

Аб'ект кантэксту толькі для чытання, які перадаецца ў `before()` і `after()`:

| Уласцівасць | Тып | Апісанне |
|-------------|-----|----------|
| `$target` | `string` | Поўная назва мэты (`App\Service::method` або `my_function`) |
| `$class` | `string` | Назва класа або `""` для функцый |
| `$method` | `string` | Назва метада або `""` для функцый |
| `$function` | `string` | Назва функцыі для `TARGET_FUNCTION` або `""` для метадаў |
| `$objectId` | `int` | `spl_object_id` для метадаў, `0` для функцый |
| `$requestId` | `string` | Ідэнтыфікатар бягучага запыту |
| `$traceId` | `string` | W3C trace ID (калі трасіроўка ўключана) |

| Метад | Вяртае | Апісанне |
|-------|--------|----------|
| `getParams()` | `array` | Аргументы, перададзеныя ў дэкараваную функцыю (ленівы, нулявы кошт, калі не выклікаецца) |
| `getResult()` | `mixed` | Вяртаемае значэнне дэкараванай функцыі (толькі ў `after()`, вяртае `null` у `before()`) |
| `hasResult()` | `bool` | `true` у `after()`, калі функцыя завершылася паспяхова, `false` у адваротным выпадку |

### `OxPHP\Decorator\RejectedException`

Выключэнне, якое выкідваецца, калі Rust-натыўны дэкаратар адхіляе выклік праз `DecoratorAction::Reject`. Пашырае `\Exception`.

### `oxphp_register_decorator()`

```php
oxphp_register_decorator(string $class): bool
```

Рэгіструе PHP-клас як дэкаратар. Клас павінен рэалізаваць `OxPHP\Decorator\AttributeInterface` і быць пазначаны `#[Attribute(...)]`. Вяртае `true` пры поспеху, `false` з `E_WARNING` пры памылцы валідацыі.

## Мэты атрыбутаў

Атрыбуты дэкаратараў не абавязаны падтрымліваць усе мэты. Кожны клас дэкаратара аб'яўляе свае мэты праз PHP-атрыбут `#[Attribute(...)]`:

```php
// Толькі метады
#[Attribute(Attribute::TARGET_METHOD)]
class RequireAuth implements OxPHP\Decorator\AttributeInterface { ... }

// Толькі класы — before()/after() выконваецца для кожнага метада класа
#[Attribute(Attribute::TARGET_CLASS)]
class Audited implements OxPHP\Decorator\AttributeInterface { ... }

// Усе тры
#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
class Timer implements OxPHP\Decorator\AttributeInterface { ... }
```

PHP праверае мэты падчас кампіляцыі. Размяшчэнне атрыбута там, дзе яго флагі мэты не дазваляюць, прыводзіць да PHP-памылкі да выканання любой логікі дэкаратара.

### Семантыка TARGET_CLASS

Калі атрыбут дэкаратара размяшчаецца на класе, сістэма выклікае `before()`/`after()` для **кожнага выкліку метада** гэтага класа. Сам дэкаратар вырашае, што рабіць — замяраць час кожнага метада, адсочваць час жыцця аб'екта або нешта іншае:

```php
#[Timer]
class PaymentProcessor {
    public function charge() { ... }  // Timer выконваецца
    public function refund() { ... }  // Timer выконваецца
}
```

Дэкаратар у стылі жыццёвага цыклу можа фільтраваць па назве метада:

```php
public function before(OxPHP\Decorator\Context $ctx): void {
    if ($ctx->method === '__construct') {
        // пачаць адсочванне часу жыцця аб'екта
    }
}
```

## Парадак выканання

Калі прымяняюцца некалькі дэкаратараў, яны выконваюцца ў **парадку атрыбутаў** (зверху ўніз), а `after()` — у адваротным парадку:

```php
#[DecoratorA]
#[DecoratorB]
function foo() { ... }
```

```
A.before() → B.before() → foo() → B.after() → A.after()
```

Гэта стэкавая семантыка — знешні дэкаратар бачыць поўнае выкананне, уключаючы ўнутраныя дэкаратары.

## Паўтаральныя атрыбуты

Дэкаратары могуць быць пазначаны `IS_REPEATABLE`, дазваляючы некалькі экзэмпляраў на адной мэце. Кожны атрымлівае ўласны кэшаваны экзэмпляр з уласнымі аргументамі канструктара:

```php
#[Attribute(Attribute::TARGET_METHOD | Attribute::IS_REPEATABLE)]
class Notify implements OxPHP\Decorator\AttributeInterface { ... }

class OrderService {
    #[Notify(channel: 'slack')]
    #[Notify(channel: 'email')]
    public function placeOrder(): void { ... }
}
```

## Апрацоўка выключэнняў

| Сцэнарый | Паводзіны |
|----------|-----------|
| `before()` выкідвае выключэнне | Функцыя НЕ выконваецца. Папярэднія паспяховыя дэкаратары атрымліваюць `after()` у адваротным парадку (ачыстка). |
| Функцыя выкідвае выключэнне | `after()` ВЫКЛІКАЕЦЦА для ўсіх дэкаратараў. `$ctx->hasResult()` вяртае `false`. |
| `after()` выкідвае выключэнне | Распаўсюджваецца да выкліку. `after()` астатніх дэкаратараў прапускаецца. |

## Rust Plugin API

Плагіны могуць рэгістраваць дэкаратары на Rust праз трэйт `Decorator`. Яны больш эфектыўныя, чым PHP-дэкаратары — без накладных выдаткаў на стварэнне PHP-аб'ектаў або дыспетчарызацыю метадаў.

```rust
use oxphp::decorator::{Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult, AttributeTargets};

struct TimerDecorator;

impl Decorator for TimerDecorator {
    fn attribute_name(&self) -> &str { "App\\Profiler\\Timer" }
    fn targets(&self) -> AttributeTargets { AttributeTargets::ALL }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        // пачаць вымярэнне часу
        DecoratorAction::Continue
    }

    fn on_end(&self, ctx: &DecoratorCallContext, result: &DecoratorCallResult) {
        // запісаць прошлы час
    }
}
```

Рэгістрацыя падчас ініцыялізацыі плагіна:

```rust
fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_decorator(TimerDecorator);
    Ok(())
}
```

Rust- і PHP-дэкаратары трапляюць у адзін `DecoratorRegistry` і суіснуюць на адных функцыях.

## Прадукцыйнасць

Сістэма дэкаратараў распрацавана з мінімальнымі накладнымі выдаткамі:

- **Нулявы кошт для недэкараваных функцый** — ініцыялізатар назіральніка вяртае `{NULL, NULL}` для функцый без зарэгістраваных атрыбутаў дэкаратараў. PHP кэшуе гэты вынік на кожны `op_array`, таму наступныя выклікі цалкам прапускаюць праверку.
- **Аднаразовае вырашэнне** — супастаўленне атрыбутаў з дэкаратарамі адбываецца адзін раз на функцыю (пры першым выкліку), а не пры кожным выкліку.
- **Кэшаванне экзэмпляраў** — PHP-аб'екты дэкаратараў стваруюцца адзін раз для кожнай пары функцыя-дэкаратар (з аргументамі канструктара, прачытанымі з атрыбута), затым кэшуюцца ў TLS кожнага патоку на час запыту (або воркера). Пры наступных выкліках аб'екты не ствараюцца.
- **Паўторнае выкарыстанне радкоў `Arc<str>`** — радкі target/class/method размяшчаюцца аднойчы падчас вырашэння і перадаюцца ў усіх выкліках праз падлік спасылак.
- **Rust-дэкаратары цалкам мінуюць накладныя выдаткі PHP** — `on_begin()`/`on_end()` выклікаюцца напрамую праз FFI без стварэння PHP-аб'ектаў, дыспетчарызацыі метадаў або маніпуляцый з zval.

## Глядзіце таксама

- [Функцыі PHP](../php/functions.md) — даведнік па `oxphp_register_decorator()`
- [Сістэма падзей](../architecture/event-system.md) — дыспетчарызацыя падзей (дэкаратары працуюць на ўзроўні функцыі, а не запыту)
- [SAPI і мост](../architecture/sapi-bridge.md) — C-мост, які злучае PHP і Rust
