---
title: OPcache и JIT
description: Настройка OPcache и JIT-компиляции для оптимальной производительности PHP с OxPHP, включая предзагрузку и параметры для разработки.
---

# OPcache и JIT

OPcache работает с OxPHP без какой-либо дополнительной настройки. Все PHP-воркер-потоки используют единый сегмент памяти OPcache — скрипты компилируются один раз при первом выполнении, а затем обслуживаются из кэша всеми воркерами. Никакой специальной настройки для организации этого совместного использования не требуется.

## Как OPcache работает с OxPHP

OxPHP регистрируется как именованный SAPI, и OPcache обрабатывает его идентично другим серверным SAPI. Ключевые характеристики:

- **Общий кэш для всех воркеров**: все PHP-воркер-потоки используют одинаковый скомпилированный кэш опкодов. Один воркер компилирует файл — все воркеры выигрывают.
- **Отсутствие компиляции на каждый запрос**: после первого запроса к скрипту все последующие запросы полностью пропускают этапы разбора и компиляции.
- `opcache.enable_cli` не имеет значения — этот параметр применяется только к SAPI `cli` и `phpdbg`. OxPHP не является ни тем, ни другим.

Для включения OPcache достаточно минимальной конфигурации:

```ini
zend_extension=opcache

[opcache]
opcache.enable=1
```

## Рекомендуемые настройки для продакшена

Эти настройки оптимизированы для продакшен-развёртываний в контейнерах, где PHP-файлы не изменяются во время выполнения. Отключите проверку временных меток и предзагрузите скомпилированные файлы при запуске для максимальной производительности.

```ini
zend_extension=opcache

[opcache]
opcache.enable=1
opcache.memory_consumption=128
opcache.interned_strings_buffer=16
opcache.max_accelerated_files=10000
opcache.validate_timestamps=0
opcache.revalidate_freq=0
opcache.file_update_protection=0
opcache.jit_buffer_size=64M
opcache.jit=tracing
```

| Параметр | Рекомендуемое значение | Описание |
|---------|----------------------|---------|
| `memory_consumption` | `128` | Объём разделяемой памяти в МБ для скомпилированных скриптов. Увеличьте, если `opcache_get_status()` показывает низкий объём свободной памяти. |
| `interned_strings_buffer` | `16` | Объём памяти в МБ для интернированных строк, используемых всеми воркерами. |
| `max_accelerated_files` | `10000` | Максимальное количество кэшируемых скриптов. Устанавливайте значение выше общего количества `.php`-файлов. |
| `validate_timestamps` | `0` | При значении `0` OPcache никогда не проверяет файловую систему на наличие изменений. Перезапустите контейнер или вызовите `opcache_reset()` для применения изменений кода. |
| `revalidate_freq` | `0` | Интервал между проверками файловой системы в секундах. Не имеет эффекта при `validate_timestamps=0`. |
| `file_update_protection` | `0` | Время в секундах после изменения файла до его допуска к кэшированию. Установите `0` для немедленного кэширования при запуске. |

## Настройки для разработки

При разработке включайте проверку временных меток, чтобы изменения кода вступали в силу без перезапуска контейнера. Отключите JIT для получения более понятных трассировок стека при отладке.

```ini
zend_extension=opcache

[opcache]
opcache.enable=1
opcache.memory_consumption=128
opcache.interned_strings_buffer=16
opcache.max_accelerated_files=10000
opcache.validate_timestamps=1
opcache.revalidate_freq=2
opcache.jit_buffer_size=0
opcache.jit=disable
```

При `validate_timestamps=1` OPcache проверяет время изменения файлов каждые `revalidate_freq` секунд. Это добавляет небольшие накладные расходы на каждый запрос, но позволяет редактировать PHP-файлы и видеть изменения при следующем запросе.

## JIT-компиляция

JIT-компилятор OPcache транслирует PHP-опкоды в нативный машинный код во время выполнения. Используйте режим `tracing` для наилучшей оптимизации:

```ini
opcache.jit=tracing
opcache.jit_buffer_size=64M
```

JIT приносит наибольшую пользу для CPU-интенсивного PHP-кода — вычислительно насыщенных циклов, обработки строк, работы с изображениями и рендеринга шаблонов. Для I/O-интенсивных приложений, которые большую часть времени ожидают ответа от базы данных или внешних API, улучшение минимально.

Для отключения JIT:

```ini
opcache.jit=disable
opcache.jit_buffer_size=0
```

## Предзагрузка

Предзагрузка OPcache компилирует и кэширует PHP-файлы при запуске сервера, до обработки каких-либо запросов. Это полностью устраняет затраты на компиляцию при первом запросе и делает классы и функции глобально доступными без накладных расходов на `require` или автозагрузчик.

Настройте предзагрузку в INI-файле:

```ini
opcache.preload=/var/www/html/preload.php
opcache.preload_user=www-data
```

Создайте скрипт `preload.php`, который загружает наиболее часто используемые файлы:

```php
<?php
// preload.php — выполняется один раз при запуске сервера

require __DIR__ . '/vendor/autoload.php';

// Предзагружаем основные файлы фреймворка
$files = glob(__DIR__ . '/vendor/symfony/http-kernel/**.php');
foreach ($files as $file) {
    opcache_compile_file($file);
}

// Предзагружаем наиболее используемые пути приложения
opcache_compile_file(__DIR__ . '/src/Controller/ApiController.php');
opcache_compile_file(__DIR__ . '/src/Service/UserService.php');
```

> **Примечание:** Предзагруженные классы и функции постоянно доступны для всех запросов. Они не могут быть изменены без перезапуска сервера.

## Применение конфигурации PHP

OxPHP читает конфигурацию PHP из стандартной директории `conf.d`. Используйте Docker-том или инструкцию `COPY` для подключения вашего INI-файла.

**Запуск через docker run:**

```bash
docker run -p 8080:8080 \
  -v ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro \
  ghcr.io/oxphp/oxphp:0.1.0
```

**Dockerfile:**

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY oxphp.ini /usr/local/etc/php/conf.d/oxphp.ini
COPY --chown=www-data:www-data . /var/www/html
```

**Docker Compose:**

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "8080:8080"
    volumes:
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
      - ./src:/var/www/html
```

## Мониторинг состояния кэша

Проверьте состояние OPcache из PHP, чтобы убедиться в его работоспособности:

```php
<?php
$status = opcache_get_status();

echo "Cached scripts: " . $status['opcache_statistics']['num_cached_scripts'] . "\n";
echo "Cache hits: "     . $status['opcache_statistics']['hits'] . "\n";
echo "Cache misses: "   . $status['opcache_statistics']['misses'] . "\n";
echo "Free memory: "    . $status['memory_usage']['free_memory'] . " bytes\n";
```

Если `free_memory` постоянно мало, увеличьте `opcache.memory_consumption`.

## См. также

- [Руководство по Docker](../getting-started/docker.md) — настройка контейнера и подключение конфигурационных файлов
- [Справочник по конфигурации](../operations/configuration.md) — переменные окружения для OxPHP
- [Режим Worker](../features/worker-mode.md) — постоянные PHP-процессы, которые больше всего выигрывают от OPcache
