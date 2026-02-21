---
title: Усталяванне
description: Як усталяваць і запусціць OxPHP
---

## Docker-выява (рэкамендуецца)

OxPHP распаўсюджваецца як гатовая Docker-выява. Сцягніце апошні начны зборнік:

```bash
docker pull ghcr.io/oxphp/oxphp:nightly
```

Стварыце `Dockerfile` у каранёвым каталогу свайго праекта:

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data ./src /var/www/html
```

Зберыце і запусціце:

```bash
docker build -t my-app .
docker run -p 8080:8080 my-app
```

Усё. Выява ўключае бінарны файл Rust, асяроддзе выканання PHP 8.4 ZTS, бібліятэку моста, PHP-пашырэнне і ўсе неабходныя залежнасці. Інструменты зборкі не патрэбны.

## Папярэднія патрабаванні

**Docker (рэкамендуецца):**

- Docker Engine 20.10+ або Docker Desktop

**Зборка з зыходных кодаў (без PHP):**

- Набор інструментаў Rust 1.75+ (рэкамендуецца `rustup`)

**Зборка з зыходных кодаў (з PHP):**

- Набор інструментаў Rust 1.75+
- PHP 8.4 з уключаным ZTS (Zend Thread Safety)
- `libphp.so`, даступная ў шляху пошуку бібліятэк
- C-кампілятар (gcc або clang) для бібліятэкі моста і PHP-пашырэння

## Зборка з зыходных кодаў (Stub Executor)

Каб сабраць OxPHP без падтрымкі PHP (толькі абслугоўванне статычных файлаў, карысна для распрацоўкі), выкарыстоўвайце `--no-default-features` для адключэння магчымасці `php`:

```bash
cargo build --release --no-default-features
```

Выніковы бінарны файл знаходзіцца па шляху `target/release/oxphp`. Ён выкарыстоўвае stub executor, які вяртае адказ-загальнік для PHP-запытаў.

**Заўвага:** Магчымасць `php` уключана па змаўчанні. Выкананне `cargo build --release` без `--no-default-features` патрабуе наяўнасці `libphp.so` і бібліятэкі моста на хасце.

## Зборка з зыходных кодаў (з PHP)

Зборка з PHP патрабуе ўсталёўкі `libphp.so` (зборка ZTS) і бібліятэкі моста на хасце:

```bash
# Зборка і ўсталёўка бібліятэкі моста
cd ext/bridge
make && sudo make install

# Зборка і ўсталёўка PHP-пашырэння
cd ext
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# Зборка OxPHP з падтрымкай PHP (магчымасць php уключана па змаўчанні)
cargo build --release
```

Падчас выканання бінарны файл патрабуе `libphp.so` і `liboxphp_bridge.so` у шляху пошуку бібліятэк:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

### Сумяшчальнасць з Alpine

Калі вы разгортваеце на Alpine Linux, неабходна сабраць бінарны файл Rust унутры той жа выявы `php:8.4-zts-alpine`, якая выкарыстоўваецца для асяроддзя выканання PHP. Зборка ў асобнай выяве або на іншым libc (glibc супраць musl) выклікае пашкоджанне TLS падчас выканання. Прыкладзены Dockerfile апрацоўвае гэта правільна.

## Запуск тэстаў

Запусціце набор тэстаў на хасце без PHP, адключыўшы магчымасці па змаўчанні:

```bash
# Усе праверкі (фарматаванне, аналіз, тэсты)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Толькі модульныя тэсты
cargo test --no-default-features --lib

# Усе тэсты (модульныя + інтэграцыйныя)
cargo test --no-default-features

# З прыкладным плагінам
cargo clippy --no-default-features --features plugin-example -- -D warnings && cargo test --no-default-features --features plugin-example
```

## Праверка ўсталёўкі

Пасля запуску OxPHP вы павінны ўбачыць структураваны JSON-вывад журнала:

```
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:8080",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:8080"}
```

Праверце, ці адказвае сервер:

```bash
curl http://localhost:8080/
```

Калі вы наладзілі ўнутраны сервер, праверце эндпойнт здароўя:

```bash
curl http://localhost:9090/health
```

## Гл. таксама

- [Хуткі старт](quick-start.md) -- запусціце OxPHP менш чым за 5 хвілін
- [Docker](docker.md) -- даведнік па compose.yml, этапы Dockerfile і парады па разгортванні
- [Канфігурацыя](../operations/configuration.md) -- поўны спіс зменных асяроддзя
