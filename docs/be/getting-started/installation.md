---
title: Усталяванне
description: Як усталяваць і сабраць OxPHP
---

## Перадумовы

**Docker (рэкамендавана):**

- Docker Engine 20.10+ або Docker Desktop
- Docker Compose v2

**Зборка з зыходнікаў (без PHP):**

- Набор інструментаў Rust 1.75+ (рэкамендуецца `rustup`)

**Зборка з зыходнікаў (з PHP):**

- Набор інструментаў Rust 1.75+
- PHP 8.4 з уключаным ZTS (Zend Thread Safety)
- `libphp.so` даступная ў шляху пошуку бібліятэк
- Кампілятар C (gcc або clang) для бібліятэкі моста і PHP-пашырэння

## Зборка з Docker

Docker -- гэта асноўны метад зборкі. Ён стварае мінімальны вобраз Alpine з бінарным файлам Rust, рантаймам PHP, бібліятэкай моста і папярэдне сканфігураваным PHP-пашырэннем.

```bash
docker compose build
docker compose up -d
```

Шматстадыйны Dockerfile апрацоўвае поўны канвеер зборкі:

1. Кампілюе бібліятэку моста на C (`liboxphp_bridge.so`)
2. Збірае PHP-пашырэнне (`oxphp_sapi.so`) для PHP 8.4 ZTS
3. Збірае бінарны файл Rust у тым самым вобразе `php:8.4-zts-alpine`
4. Капіруе толькі рантайм-артэфакты ў лёгкі вобраз Alpine

Каб уключыць дадатковыя магчымасці, такія як плагін адладкі, перадайце `CARGO_FEATURES` як аргумент зборкі:

```bash
docker compose build --build-arg CARGO_FEATURES="plugin-debug"
```

Глядзіце [даведнік па Docker](/getting-started/docker/) для поўнага разбору стадый Dockerfile і канфігурацыі `docker-compose.yml`.

## Зборка з зыходнікаў (Stub Executor)

Каб сабраць OxPHP без падтрымкі PHP (толькі абслугоўванне статычных файлаў, карысна для распрацоўкі), выкарыстоўвайце `--no-default-features` для адключэння магчымасці `php`:

```bash
cargo build --release --no-default-features
```

Выніковы бінарны файл знаходзіцца ў `target/release/oxphp`. Ён выкарыстоўвае stub executor, які вяртае адказ-заглушку для PHP-запытаў.

**Заўвага:** Магчымасць `php` уключана па змаўчанні. Запуск `cargo build --release` без `--no-default-features` патрабуе наяўнасці `libphp.so` і бібліятэкі моста на хасце.

## Зборка з зыходнікаў (з PHP)

Зборка з PHP патрабуе ўсталяванай на хасце `libphp.so` (зборка ZTS) і бібліятэкі моста:

```bash
# Зборка і ўсталяванне бібліятэкі моста
cd ext/bridge
make && sudo make install

# Зборка і ўсталяванне PHP-пашырэння
cd ext
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# Зборка OxPHP з падтрымкай PHP (магчымасці па змаўчанні ўключаюць php)
cargo build --release
```

Падчас выканання бінарны файл патрабуе `libphp.so` і `liboxphp_bridge.so` у шляху пошуку бібліятэк:

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

### Сумяшчальнасць з Alpine

Калі вы разгортваеце на Alpine Linux, бінарны файл Rust трэба збіраць у тым самым вобразе `php:8.4-zts-alpine`, які выкарыстоўваецца для рантайму PHP. Зборка ў іншым вобразе або з іншай libc (glibc супраць musl) выклікае пашкоджанне TLS падчас выканання. Пастаўлены Dockerfile апрацоўвае гэта правільна.

## Запуск тэстаў

Запуск набору тэстаў на хасце без PHP з адключэннем магчымасцяў па змаўчанні:

```bash
# Усе праверкі (фарматаванне, лінтынг, тэсты)
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# Толькі модульныя тэсты
cargo test --no-default-features --lib

# Усе тэсты (модульныя + інтэграцыйныя)
cargo test --no-default-features

# З плагінам адладкі
cargo clippy --no-default-features --features plugin-debug -- -D warnings && cargo test --no-default-features --features plugin-debug
```

## Праверка ўсталявання

Пасля запуску OxPHP вы павінны ўбачыць структураваны JSON-лог:

```
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:8080",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:8080"}
```

Праверце, што сервер адказвае:

```bash
curl http://localhost:8080/
```

Калі вы наладзілі ўнутраны сервер, праверце эндпойнт стану:

```bash
curl http://localhost:9090/health
```

## Глядзіце таксама

- [Хуткі старт](/getting-started/quick-start/) -- запусціце OxPHP менш чым за 5 хвілін
- [Docker](/getting-started/docker/) -- стадыі Dockerfile, даведнік docker-compose.yml і парады па разгортванні
- [Канфігурацыя](/operations/configuration/) -- поўны спіс зменных асяроддзя
