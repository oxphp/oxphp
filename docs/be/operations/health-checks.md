---
title: Праверкі стану
description: Канчатковыя кропкі ўнутранага сервера для маніторынгу стану і аркестрацыі кантэйнераў
---

OxPHP запускае ўнутраны HTTP-сервер на асобным порце для праверак стану, метрык і інспекцыі канфігурацыі. Гэты сервер ізаляваны ад асноўнага порта трафіку, каб маніторынгавы трафік не канкурыраваў з запытамі праграмы.

## Уключэнне ўнутранага сервера

Усталюйце зменную асяроддзя `INTERNAL_ADDR`, каб запусціць унутраны сервер:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Калі гэта зменная не ўсталявана, унутраны сервер не запускаецца.

## Канчатковыя кропкі

### `GET /health`

Вяртае стан здароўя сервера ў фармаце JSON.

```bash
curl http://localhost:9090/health
```

**Адказ (здаровы):**

```json
{
  "status": "ok",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": true,
  "plugins": {}
}
```

**Адказ (дэградаваны):**

```json
{
  "status": "degraded",
  "uptime_secs": 3612,
  "total_requests": 48203,
  "active_connections": 7,
  "executor_healthy": false,
  "plugins": {
    "example_plugin": "failed"
  }
}
```

| Поле | Тып | Апісанне |
|-------|------|-------------|
| `status` | `string` | `"ok"`, калі ўсе падсістэмы здаровыя, `"degraded"` у адваротным выпадку |
| `uptime_secs` | `integer` | Секунды з моманту запуску сервера |
| `total_requests` | `integer` | Агульная колькасць HTTP-запытаў, апрацаваных на асноўным порце |
| `active_connections` | `integer` | Бягучыя адкрытыя злучэнні на асноўным порце |
| `executor_healthy` | `boolean` | Ці прымае пул PHP-воркераў запыты |
| `plugins` | `object` | Стан здароўя кожнага загружанага плагіна. Значэнні: `"healthy"` або `"failed"` |

**Коды стану HTTP:**

| Код | Значэнне |
|------|---------|
| `200 OK` | Выканаўца і ўсе плагіны здаровыя |
| `503 Service Unavailable` | Выканаўца або любы плагін паведамляе пра збой |

Праверка `executor_healthy` выклікае метад `is_healthy()` на PHP-выканаўцы. Калі пул воркераў спыніўся або іншым чынам не можа апрацоўваць запыты, гэта вяртае `false`. Акрамя таго, калі любы плагін паведамляе пра стан `Failed`, агульны стан — `"degraded"`, і канчатковая кропка вяртае 503.

### `GET /metrics`

Вяртае метрыкі ў фармаце тэкставай экспазіцыі, сумяшчальным з Prometheus. Глядзіце старонку [Метрыкі](metrics.md) для поўнай даведкі па метрыках. Плагіны могуць дадаваць дадатковыя метрыкі да гэтага вываду.

```bash
curl http://localhost:9090/metrics
```

### `GET /config`

Вяртае актыўную канфігурацыю сервера ў фармаце JSON. Канфідэнцыяльныя значэнні (шляхі да TLS-ключоў) рэдагуюцца. Канфігурацыя плагінаў уключана пад ключом `plugins`.

```bash
curl http://localhost:9090/config
```

```json
{
  "listen_addr": "0.0.0.0:8080",
  "document_root": "/var/www/html/public",
  "index_file": "index.php",
  "executor_type": "sapi",
  "max_connections": 10000,
  "drain_timeout_secs": 30,
  "header_timeout_secs": 5,
  "idle_timeout_secs": 60,
  "request_timeout_secs": 120,
  "rate_limit": 100,
  "rate_window": 60,
  "tls_enabled": true,
  "error_pages_dir": "/etc/oxphp/error-pages",
  "compression": true,
  "access_log": true,
  "plugins": {}
}
```

### Унутраныя маршруты плагінаў

Шляхі, якія пачынаюцца з `/__`, зарэзерваваны для ўнутраных канчатковых кропак, вызначаных плагінамі. Калі ніводзін плагін не апрацоўвае шлях, вяртаецца адказ `404 Not Found`.

Любы іншы шлях вяртае `404 Not Found`.

## Праверкі стану кантэйнераў

### Docker

```yaml
# docker-compose.yml
services:
  oxphp:
    image: oxphp:latest
    environment:
      INTERNAL_ADDR: "127.0.0.1:9090"
    healthcheck:
      test: ["CMD", "wget", "-qO-", "http://127.0.0.1:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

### Dockerfile HEALTHCHECK

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
  CMD wget -qO- http://127.0.0.1:9090/health || exit 1
```

### Kubernetes

```yaml
# Праверка жыццяздольнасці — перазапускае пад, калі сервер не адказвае
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 10
  failureThreshold: 3

# Праверка гатоўнасці — выдаляе пад з сэрвісу, калі стан дэградаваны
readinessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 2
  periodSeconds: 5
  failureThreshold: 2
```

Для Kubernetes выкарыстоўвайце поле `executor_healthy` і код стану HTTP для кіравання гатоўнасцю. Адказ `503` азначае, што пул PHP-воркераў або плагін дэградаваны, і пад павінен быць выдалены са спісу канчатковых кропак сэрвісу, пакуль ён не аднавіцца.

## Інтэграцыя з балансіроўшчыкам нагрузкі

Большасць балансіроўшчыкаў нагрузкі падтрымліваюць HTTP-праверкі стану. Накіруйце іх на ўнутраны порт:

| Балансіроўшчык нагрузкі | Мэта праверкі стану |
|---------------|-------------------|
| AWS ALB/NLB | `http://instance:9090/health` |
| HAProxy | `option httpchk GET /health` на порце 9090 |
| nginx upstream | `proxy_pass http://backend:9090/health` |
| Traefik | `traefik.http.services.oxphp.loadbalancer.healthcheck.path=/health` |

Канчатковая кропка `/health` лёгкая — яна чытае атамарныя лічыльнікі і выклікае `is_healthy()` на выканаўцы. Няма дыскавага ўводу-вываду, доступу да базы даных або выканання PHP.

## Меркаванні бяспекі

Унутраны сервер прывязваецца да `127.0.0.1` па змаўчанні, што робіць яго даступным толькі з лакальнай машыны. Калі вам трэба адкрыць яго для сеткі маніторынгу, прывяжыце да канкрэтнага інтэрфейсу:

```bash
# Даступны з сеткі маніторынгу
INTERNAL_ADDR=10.0.1.5:9090
```

**Не** прывязвайце ўнутраны сервер да `0.0.0.0` у вытворчасці, калі ён не знаходзіцца за файрволам або сеткавай палітыкай, якая абмяжоўвае доступ. Канчатковая кропка `/config` раскрывае аперацыйныя дэталі, якія не павінны быць публічнымі.

## Глядзіце таксама

- [Метрыкі](metrics.md) --- поўная даведка па метрыках, сумяшчальных з Prometheus
- [Канфігурацыя](configuration.md) --- усе зменныя асяроддзя і іх значэнні па змаўчанні
- [Плаўная спынка](graceful-shutdown.md) --- як праверкі стану ўзаемадзейнічаюць з адводам злучэнняў пры спынцы
