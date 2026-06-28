---
title: Примеры развёртываний
description: Готовые рецепты запуска популярных PHP-фреймворков и CMS на OxPHP — Laravel, Symfony, Yii3, WordPress, Drupal, Craft, Magento, OpenCart и October CMS — каждый с Dockerfile, Compose-файлом, шагами установки и важными нюансами, специфичными для OxPHP.
---

# Примеры развёртываний

Эти руководства показывают, как запустить девять популярных PHP-приложений на OxPHP, каждое как самодостаточный проект Docker Compose. Каждый рецепт собран и проверен от начала до конца: витрина, админ-панель, статические ресурсы и внутренний health-эндпоинт OxPHP — всё отвечает `200`.

Каждая страница — это законченный рецепт, который можно скопировать целиком: `Dockerfile`, `docker-compose.yml`, команды установки и специфичные для OxPHP детали, которые штатная документация приложения (написанная под nginx + PHP-FPM) не охватывает.

## Приложения

| Приложение | Тип | Routing mode | PHP | Дополнительные сервисы | Метод установки |
|-------------|------|--------------|-----|----------------|----------------|
| [Laravel](framework/laravel.md) | Framework | Framework | 8.5 | MySQL | `composer create-project` |
| [Symfony](framework/symfony.md) | Framework | Framework | 8.5 | — | `composer create-project` |
| [Yii3](framework/yii3.md) | Framework | Framework | 8.5 | — | `composer create-project` |
| [WordPress](cms/wordpress.md) | CMS | Traditional | 8.5 | MySQL | WP-CLI |
| [Drupal](cms/drupal.md) | CMS | Framework | 8.4 | MySQL | `drush site:install` |
| [Craft CMS](cms/craft.md) | CMS | Framework | 8.5 | MySQL | `craft install` |
| [October CMS](cms/october.md) | CMS | Framework + mirror | 8.4 | MySQL | `october:migrate` + mirror |
| [Magento](ecommerce/magento.md) | E-commerce | Framework | 8.4 | MySQL + OpenSearch | `bin/magento setup:install` |
| [OpenCart](ecommerce/opencart.md) | E-commerce | Traditional | 8.4 | MySQL | CLI-установщик |

## Что общего у всех рецептов

### Сборка на базе опубликованного образа OxPHP

OxPHP поставляет готовый PHP-рантайм как `ghcr.io/oxphp/oxphp` (по умолчанию PHP 8.5; вариант с PHP 8.4 публикуется как `ghcr.io/oxphp/oxphp:<ver>-php8.4-alpine<X>`). Опубликованный образ уже содержит бинарник `oxphp`, `libphp.so`, SAPI-расширение OxPHP, PHP CLI и удобный для Composer инструментарий. Рецепты расширяют его одним из двух способов:

1. **Копирование OxPHP в базовый образ `php:*-zts-alpine`** (используется в Laravel, Symfony, Yii3, Craft, Magento, OpenCart, Drupal, October). Многоступенчатая сборка собирает образ `dev` из четырёх стадий:

   ```dockerfile
   FROM php:8.4-zts-alpine3.23 AS php-base       # PHP-расширения вашего приложения
   FROM composer:2            AS composer        # бинарник Composer
   FROM ghcr.io/oxphp/oxphp:0.9.0-php8.4-alpine3.23 AS oxphp   # артефакты OxPHP
   FROM php-base AS dev                          # итоговый образ
   # ... копируем бинарник oxphp, библиотеку bridge и SAPI-расширение:
   COPY --from=oxphp /usr/local/bin/oxphp              /usr/local/bin/oxphp
   COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
   COPY --from=oxphp /usr/local/lib/php/extensions/    /tmp/oxphp-ext/
   RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(php -r 'echo ini_get("extension_dir");')/" \
       && echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini
   ```

2. **Расширение рантайма OxPHP напрямую** (используется в WordPress). Стадия-сборщик компилирует расширения на соответствующем `php:*-zts-alpine` и кладёт `.so`-файлы в образ OxPHP.

В любом случае **ABI PHP должен совпадать**. `libphp.so` и `oxphp_sapi.so` из образа OxPHP скомпилированы под одну версию PHP (например, 8.4 → `no-debug-zts-20240924`); стадия `php-base`/сборщика должна использовать тот же `php:<X.Y>-zts-alpine<Z>`, чтобы компилируемые расширения были ABI-совместимы. Смешивание версий приводит к тому, что `oxphp_sapi.so` отказывается загружаться или повреждает musl TLS при старте.

См. [Руководство по Docker](../getting-started/docker.md) для канонического многоступенчатого шаблона и [`examples/dockerfile/`](../../../examples/dockerfile/) в репозитории для готовой к копированию версии.

### Осознанно выбирайте версию PHP

Образ по умолчанию `ghcr.io/oxphp/oxphp:<ver>` — это **PHP 8.5**. Это подходит для современных фреймворков (Laravel, Symfony, Yii3, Craft). Более старые или консервативные кодовые базы — Magento, OpenCart, Drupal, October CMS — закрепляют **PHP 8.4** через тег `…-php8.4-alpine…`, потому что их стек компонентов появился раньше 8.5 и выдаёт на нём deprecation-предупреждения. Каждый рецепт указывает, какую версию использует и почему.

### Выбирайте routing mode по структуре приложения

[Routing mode](../features/routing.md) OxPHP напрямую соответствует тому, как устроено приложение:

- **Framework mode** (`ENTRY_FILE=index.php`) — один front controller в каталоге `public/` (или `web/`, `pub/`); существующие статические файлы отдаются с диска, всё остальное направляется в `index.php`. Используйте для Laravel, Symfony, Yii3, Craft, Magento, Drupal, October.
- **Traditional mode** (без `ENTRY_FILE`) — несколько физических PHP-точек входа (например, `index.php` плюс каталог `admin/`), которые отдаются как реальные файлы. Используйте для WordPress и OpenCart.

### Установка через тот же контейнер

Образ `dev` несёт в себе PHP CLI и Composer (а также `drush`, `wp`, `bin/magento`, `php yii`, `php craft`, `php artisan` по необходимости), так что каждая команда установки и обслуживания выполняется внутри запущенного контейнера — без отдельного инструментария:

```bash
docker compose exec app php artisan migrate      # Laravel
docker compose exec app vendor/bin/drush cr      # Drupal
docker compose run  --rm app composer install    # любое
```

### Настройки безопасности по умолчанию, применимые везде

OxPHP бесплатно даёт вам несколько средств защиты, для которых nginx + PHP-FPM требуют явной настройки:

- [Блокировка dot-путей](../security/dot-path-blocking.md) — `.env`, `.git/`, `.htaccess` и любой другой путь с dot-сегментом возвращают `404` без какой-либо конфигурации. Именно поэтому запуск приложения из каталога, который также содержит `.env`, не приводит к его утечке.
- [Чёрный список выполнения PHP](../security/php-deny.md) (`PHP_DENY_PATHS`) — используется рецептами в traditional mode: OpenCart блокирует скрипты `system/` и `install/`; WordPress блокирует выполнение `.php` внутри `wp-content/uploads/`. (Не действует в framework mode, где произвольный `.php` никогда не выполняется напрямую.)
- [Разрешённые пути для симлинков](../security/symlink-allow-paths.md) (`SYMLINK_ALLOW_PATHS`) — используется October CMS, чтобы OxPHP следовал по симлинкам ресурсов, создаваемым `october:mirror public`, при этом по-прежнему блокируя выход за пределы через симлинки во всех остальных местах.

## Рецепты

Страницы сгруппированы по типу приложения, повторяя структуру каталогов:

```
examples/
├── framework/   # Laravel, Symfony, Yii3
├── cms/         # WordPress, Drupal, Craft, October
└── ecommerce/   # Magento, OpenCart
```

### Framework — `framework/`

- [Laravel](framework/laravel.md) — каноническое приложение в framework mode
- [Symfony](framework/symfony.md) — минимальный скелет, без базы данных
- [Yii3](framework/yii3.md) — самый лёгкий из всех; только основные расширения

### CMS — `cms/`

- [WordPress](cms/wordpress.md) — traditional mode, сборка с расширением рантайма, sidecar WP-CLI
- [Drupal](cms/drupal.md) — framework mode, PDO + `drush`
- [Craft CMS](cms/craft.md) — framework mode, установка через консоль
- [October CMS](cms/october.md) — framework mode с зеркалом `public/` и `SYMLINK_ALLOW_PATHS`

### E-commerce — `ecommerce/`

- [Magento](ecommerce/magento.md) — самый тяжёлый: OpenSearch, PHP 8.4, симлинк версии статических ресурсов
- [OpenCart](ecommerce/opencart.md) — traditional mode с двумя front controller'ами и `PHP_DENY_PATHS`
