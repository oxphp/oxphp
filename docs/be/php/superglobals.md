---
title: Суперглабальныя зменныя
description: Як OxPHP запаўняе суперглабальныя масівы PHP
---

OxPHP запаўняе ўсе стандартныя суперглабальныя зменныя PHP, каб існуючы PHP-код працаваў без змен. Кожнае значэнне ўсталёўваецца **перад** выкананнем `php_request_startup()`, таму суперглабальныя зменныя даступныя з першага радка вашага скрыпту — у тым ліку ўнутры пашырэнняў, такіх як OPcache, якія чытаюць іх падчас ініцыялізацыі запыту.

## `$_SERVER`

Карыстальніцкі SAPI рэгіструе поўны набор зменных CGI/1.1 праз зваротны выклік `register_server_variables`. Гэтыя зменныя ствараюцца з HTTP-запыту і ўводзяцца ў `$_SERVER` падчас запуску запыту PHP.

### Стандартныя зменныя CGI

| Зменная | Крыніца | Прыклад |
|----------|--------|---------|
| `REQUEST_METHOD` | HTTP-метад | `GET` |
| `REQUEST_URI` | Поўны URI з радком запыту | `/app?page=2` |
| `QUERY_STRING` | Частка запыту URI | `page=2` |
| `SERVER_PROTOCOL` | Заўсёды `HTTP/1.1` | `HTTP/1.1` |
| `SCRIPT_NAME` | Шлях URI без радка запыту | `/app` |
| `PHP_SELF` | Тое ж, што `SCRIPT_NAME` | `/app` |
| `SCRIPT_FILENAME` | Абсалютны шлях да скрыпту ў файлавай сістэме | `/var/www/html/index.php` |
| `DOCUMENT_ROOT` | Каранёвы каталог вэб-сервера | `/var/www/html` |
| `SERVER_SOFTWARE` | Ідэнтыфікатар сервера | `OxPHP/0.1.0` |
| `GATEWAY_INTERFACE` | Версія CGI | `CGI/1.1` |
| `REMOTE_ADDR` | IP-адрас кліента | `172.17.0.1` |
| `REMOTE_PORT` | Порт кліента | `54321` |
| `SERVER_NAME` | З загалоўка `Host` (частка хоста) | `example.com` |
| `SERVER_PORT` | З загалоўка `Host` (частка порта) | `8080` |
| `CONTENT_TYPE` | З загалоўка `Content-Type` | `application/json` |
| `CONTENT_LENGTH` | З загалоўка `Content-Length` | `128` |

Калі загаловак `Host` адсутнічае, `SERVER_NAME` па змаўчанні роўны `localhost`, а `SERVER_PORT` — `80`.

### Загалоўкі HTTP-запыту

Усе загалоўкі HTTP-запыту дадаюцца ў `$_SERVER` з прэфіксам `HTTP_` і злучкамі, пераўтворанымі ў падкрэсленні, згодна са стандартнымі канвенцыямі CGI:

```
Accept: text/html         → HTTP_ACCEPT = "text/html"
X-Forwarded-For: 1.2.3.4 → HTTP_X_FORWARDED_FOR = "1.2.3.4"
Authorization: Bearer ... → HTTP_AUTHORIZATION = "Bearer ..."
```

`Content-Type` і `Content-Length` **не** маюць прэфікса `HTTP_` — яны з'яўляюцца як `CONTENT_TYPE` і `CONTENT_LENGTH` непасрэдна, як патрабуе спецыфікацыя CGI.

### Прыклад выкарыстання

```php
<?php
$method  = $_SERVER['REQUEST_METHOD'];
$uri     = $_SERVER['REQUEST_URI'];
$ip      = $_SERVER['REMOTE_ADDR'];
$host    = $_SERVER['SERVER_NAME'];
$docroot = $_SERVER['DOCUMENT_ROOT'];

// Доступ да карыстальніцкіх загалоўкаў
$token   = $_SERVER['HTTP_AUTHORIZATION'] ?? '';
$xff     = $_SERVER['HTTP_X_FORWARDED_FOR'] ?? $_SERVER['REMOTE_ADDR'];
```

## `$_GET`

Параметры радка запыту разбіраюцца стандартным рухавіком PHP з серверанай зменнай `QUERY_STRING`. OxPHP усталёўвае `QUERY_STRING` з URI запыту, і PHP аўтаматычна запаўняе `$_GET` падчас запуску запыту.

```php
<?php
// Запыт: GET /search?q=oxphp&page=2
$query = $_GET['q'];     // "oxphp"
$page  = $_GET['page'];  // "2"
```

## `$_POST`

SAPI прадастаўляе зваротны выклік `read_post`, які падае цела запыту ў стандартны парсер POST PHP. PHP апрацоўвае абодва тыпы кантэнту: `application/x-www-form-urlencoded` і `multipart/form-data`. Цела чытаецца інкрыментальна — PHP выклікае зваротны выклік паўторна, пакуль ён не верне 0 байтаў.

```php
<?php
// Запыт: POST /login з целам application/x-www-form-urlencoded
$username = $_POST['username'];
$password = $_POST['password'];
```

Неапрацаванае цела запыту таксама даступна праз `php://input`:

```php
<?php
$json = json_decode(file_get_contents('php://input'), true);
```

## `$_COOKIE`

SAPI прадастаўляе зваротны выклік `read_cookies`, які вяртае неапрацаваны радок загалоўка `Cookie`. Рухавік PHP разбірае яго ў масіў `$_COOKIE` падчас запуску запыту.

```php
<?php
// Запыт з Cookie: session=abc123; theme=dark
$session = $_COOKIE['session'];  // "abc123"
$theme   = $_COOKIE['theme'];    // "dark"
```

## `$_REQUEST`

`$_REQUEST` запаўняецца PHP на аснове дырэктывы INI `request_order` (па змаўчанні: `"GP"` — GET, потым POST). Ён аб'ядноўвае `$_GET` і `$_POST` (і, апцыянальна, `$_COOKIE`) у наладжаным парадку. OxPHP не змяняе гэтых паводзінаў.

## `$_FILES`

Загрузка файлаў, адпраўленых праз `multipart/form-data`, разбіраецца стандартным механізмам `read_post` PHP. Масіў `$_FILES` запаўняецца аўтаматычна, уключаючы `name`, `type`, `tmp_name`, `error` і `size` для кожнага загружанага файла.

```php
<?php
if ($_FILES['avatar']['error'] === UPLOAD_ERR_OK) {
    $tmp  = $_FILES['avatar']['tmp_name'];
    $name = $_FILES['avatar']['name'];
    move_uploaded_file($tmp, "/uploads/$name");
}
```

## Дэталі рэалізацыі

### Пакетны шаблон FFI

OxPHP збірае ўсе пары ключ-значэнне `$_SERVER` у адзіны `Vec<(CString, CString)>` у Rust і захоўвае іх у лакальным для патоку сховішчы перад пачаткам запыту PHP. Падчас зваротнага выкліку `register_server_variables` увесь вектар ітаруецца за адзін праход, выклікаючы `php_register_variable_safe()` для кожнага запісу. Гэта дазваляе пазбегнуць выклікаў FFI для кожнай зменнай і захоўвае накладныя выдаткі пастаяннымі незалежна ад колькасці загалоўкаў.

### Патрабаванні да часу жыцця даных

Усе даныя, спецыфічныя для запыту — серверныя зменныя, радок кукі і цела запыту — павінны быць захаваны ў лакальным для патоку сховішчы **перад** выклікам `php_request_startup()`. Гэтыя значэнні павінны заставацца сапраўднымі да `php_request_shutdown()`, бо рухавік PHP захоўвае неапрацаваныя паказальнікі на іх. OxPHP захоўвае іх у структуры `RequestData` унутры `thread_local! { RefCell }` і ачышчае толькі пасля поўнага завяршэння запыту.

### Лакальны для патоку мост

C-бібліятэка маста (`liboxphp_bridge.so`) выкарыстоўвае `__thread` TLS для абмену кантэкстам запыту (ідэнтыфікатар запыту, ідэнтыфікатар воркера, час запыту) паміж Rust і PHP-пашырэннем. І бінарны файл Rust, і PHP-пашырэнне звязваюцца з адной і той жа агульнай бібліятэкай, што дае ім доступ да аднаго лакальнага для патоку сховішча. Гэта адзіны надзейны механізм для абмену станам праз межы `dlopen`.

## Глядзіце таксама

- [Функцыі PHP-пашырэння](functions.md) --- убудаваныя функцыі для доступу да ідэнтыфікатараў запытаў, інфармацыі пра воркераў і метаданых сервера
- [Сумяшчальнасць з OPcache](opcache.md) --- як час запыту забяспечвае `file_update_protection` OPcache
- [SAPI-мост](/be/architecture/sapi-bridge.md) --- C-бібліятэка маста і механізм лакальнага для патоку сховішча
- [Жыццёвы цыкл запыту](/be/architecture/request-lifecycle.md) --- як даныя запыту перадаюцца з Rust у PHP
