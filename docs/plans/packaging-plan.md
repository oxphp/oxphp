# OxPHP Packaging Plan

## Goal

Ship OxPHP as a self-contained `.deb`/`.rpm` package with vendored PHP 8.4 ZTS.
Users install extensions via `oxphp-pie` (PIE wrapper). Paths mirror official
PHP packaging with `php` → `oxphp` substitution for easy future migration.

---

## 1. Path Mapping

### Official PHP 8.4 (Debian) → OxPHP

| Category | Official PHP | OxPHP |
|----------|-------------|-------|
| **Server binary** | — | `/usr/bin/oxphp` |
| **PHP CLI binary** | `/usr/bin/php8.4` | `/usr/bin/oxphp-php` |
| **phpize** | `/usr/bin/phpize8.4` | `/usr/bin/oxphp-phpize` |
| **php-config** | `/usr/bin/php-config8.4` | `/usr/bin/oxphp-php-config` |
| **PIE wrapper** | — | `/usr/bin/oxphp-pie` |
| **libphp.so** | `/usr/lib/libphp8.4.so` | `/usr/lib/oxphp/libphp.so` |
| **Bridge library** | — | `/usr/lib/oxphp/liboxphp_bridge.so` |
| **Extensions (.so)** | `/usr/lib/php/20240924/` | `/usr/lib/oxphp/20240924/` |
| **Build files (phpize)** | `/usr/lib/php/20240924/build/` | `/usr/lib/oxphp/20240924/build/` |
| **Headers** | `/usr/include/php/20240924/` | `/usr/include/oxphp/20240924/` |
| **php.ini (base)** | `/etc/php/8.4/cli/php.ini` | `/etc/oxphp/8.4/server/php.ini` |
| **conf.d (scan dir)** | `/etc/php/8.4/cli/conf.d/` | `/etc/oxphp/8.4/server/conf.d/` |
| **mods-available** | `/etc/php/8.4/mods-available/` | `/etc/oxphp/8.4/mods-available/` |

Note: we use `server` instead of `cli` because OxPHP is a server SAPI. The CLI binary
(`oxphp-php`) shares the same config — one config for both server and CLI tools.

| Category | Official PHP | OxPHP |
|----------|-------------|-------|
| **PIE phar (cached)** | — | `/var/cache/oxphp/pie.phar` (downloaded on first use) |
| **Server env config** | — | `/etc/oxphp/oxphp.env` |
| **systemd service** | — | `/lib/systemd/system/oxphp.service` |
| **Linker config** | — | `/etc/ld.so.conf.d/oxphp.conf` |
| **Web root** | — | `/var/www/html/public/` |

SAPI subdir is `server` — OxPHP is a server SAPI (not CLI).
The CLI binary (`oxphp-php`) shares the same config directory.

### PHP ini path override

The vendored `libphp.so` from `php:8.4-zts-bookworm` has compiled-in paths:
- `PHP_CONFIG_FILE_PATH` = `/usr/local/etc/php`
- `PHP_CONFIG_FILE_SCAN_DIR` = `/usr/local/etc/php/conf.d`

We redirect to OxPHP paths via:
1. **SAPI code** (`src/php/sapi.rs`): set `php_ini_path_override` → `/etc/oxphp/8.4/server`
2. **CLI wrapper** (`oxphp-php`): `PHP_INI_SCAN_DIR=/etc/oxphp/8.4/server/conf.d`
3. **extension_dir**: set in `/etc/oxphp/8.4/server/conf.d/extension.ini`

Future: rebuild PHP from source with
`--with-config-file-path=/etc/oxphp/8.4/server`
`--with-config-file-scan-dir=/etc/oxphp/8.4/server/conf.d`
to eliminate runtime overrides.

---

## 2. Package Metadata (nfpm.yaml)

### Conflicts / Replaces / Provides

```yaml
conflicts:
  - php8.4-cli
  - php8.4-dev
  - php8.4-common
  - php8.4-fpm
  - php8.4-phpdbg
  - php-cli
  - php-dev
  - php-common
  - php-fpm

replaces:
  - php8.4-cli
  - php8.4-dev
  - php8.4-common
  - php8.4-fpm
  - php-cli
  - php-dev
  - php-common
  - php-fpm

provides:
  - php-cli
  - php-dev
  - php-common
```

### Dependencies (Debian)

```yaml
depends:
  - libxml2
  - libsqlite3-0
  - libcurl4
  - libonig5
  - libargon2-1
  - zlib1g
  - libreadline8
  - libssl3

recommends:
  - gcc
  - make
  - autoconf
  - automake
  - libtool
  - m4
```

`recommends` — build tools needed by PIE to compile extensions.
Installed by default with `apt install`, skippable with `--no-install-recommends`.

### RPM overrides

```yaml
overrides:
  rpm:
    depends:
      - libxml2
      - sqlite-libs
      - libcurl
      - oniguruma
      - libargon2
      - zlib
      - readline
      - openssl-libs
    conflicts:
      - php-cli
      - php-devel
      - php-common
      - php-fpm
    replaces:
      - php-cli
      - php-devel
      - php-common
      - php-fpm
    provides:
      - php-cli
      - php-devel
      - php-common
```

---

## 3. Package Contents

### Binaries

| Source (staging) | Destination | Mode |
|-----------------|-------------|------|
| `staging/usr/bin/oxphp` | `/usr/bin/oxphp` | 0755 |
| `staging/usr/bin/oxphp-php` | `/usr/bin/oxphp-php` | 0755 |
| `staging/usr/bin/oxphp-phpize` | `/usr/bin/oxphp-phpize` | 0755 |
| `staging/usr/bin/oxphp-php-config` | `/usr/bin/oxphp-php-config` | 0755 |
| `dist/oxphp-pie` | `/usr/bin/oxphp-pie` | 0755 |

### Libraries

| Source (staging) | Destination | Mode |
|-----------------|-------------|------|
| `staging/usr/lib/oxphp/libphp.so` | `/usr/lib/oxphp/libphp.so` | 0755 |
| `staging/usr/lib/oxphp/liboxphp_bridge.so` | `/usr/lib/oxphp/liboxphp_bridge.so` | 0755 |

### Extensions

| Source (staging) | Destination |
|-----------------|-------------|
| `staging/usr/lib/oxphp/20240924/opcache.so` | `/usr/lib/oxphp/20240924/opcache.so` |
| `staging/usr/lib/oxphp/20240924/oxphp_sapi.so` | `/usr/lib/oxphp/20240924/oxphp_sapi.so` |

### Headers (for phpize / PIE)

| Source (staging) | Destination |
|-----------------|-------------|
| `staging/usr/include/oxphp/20240924/` | `/usr/include/oxphp/20240924/` |

Includes: `main/`, `Zend/`, `TSRM/`, `ext/`, `sapi/` — all PHP development headers.

### Build files (for phpize)

| Source (staging) | Destination |
|-----------------|-------------|
| `staging/usr/lib/oxphp/20240924/build/` | `/usr/lib/oxphp/20240924/build/` |

Includes: `Makefile.global`, `php.m4`, `phpize.m4`, `ltmain.sh`, `acinclude.m4`, etc.

### Configuration (config|noreplace)

| Source | Destination | Notes |
|--------|-------------|-------|
| `staging/.../php.ini` | `/etc/oxphp/8.4/server/php.ini` | php.ini-production from builder |
| `staging/.../extension.ini` | `/etc/oxphp/8.4/server/conf.d/extension.ini` | extension_dir + opcache + oxphp_sapi |
| `dist/oxphp.env` | `/etc/oxphp/oxphp.env` | Server config (LISTEN_ADDR, etc.) |

### System files

| Source | Destination |
|--------|-------------|
| `dist/oxphp.service` | `/lib/systemd/system/oxphp.service` |
| `staging/.../oxphp.conf` | `/etc/ld.so.conf.d/oxphp.conf` |

### Directories (created by package)

```
/var/www/html/public/          owner: www-data
/etc/oxphp/8.4/server/conf.d/
/etc/oxphp/8.4/mods-available/
/usr/lib/oxphp/20240924/       (extensions installed here by PIE)
/var/cache/oxphp/              (PIE phar downloaded on first use)
```

---

## 4. Scripts

### oxphp-pie (`dist/oxphp-pie`)

```bash
#!/bin/sh
set -e

PIE_PHAR="/var/cache/oxphp/pie.phar"
PHP="/usr/bin/oxphp-php"

# Download PIE on first use
if [ ! -f "$PIE_PHAR" ]; then
    echo "Downloading PIE (PHP Installer for Extensions)..."
    mkdir -p "$(dirname "$PIE_PHAR")"
    LATEST=$(curl -sI https://github.com/php/pie/releases/latest | grep -i '^location:' | sed 's|.*/v||;s/\r//')
    curl -sL "https://github.com/php/pie/releases/download/v${LATEST}/pie.phar" -o "$PIE_PHAR"
    chmod 755 "$PIE_PHAR"
    echo "PIE v${LATEST} installed."
fi

# Handle pie-install command (install/replace specific PIE version)
if [ "$1" = "pie-update" ]; then
    VERSION="${2:?Usage: oxphp-pie pie-update <version>}"
    echo "Installing PIE v${VERSION}..."
    curl -sL "https://github.com/php/pie/releases/download/v${VERSION}/pie.phar" -o "$PIE_PHAR"
    chmod 755 "$PIE_PHAR"
    echo "PIE v${VERSION} installed."
    exit 0
fi

exec "$PHP" "$PIE_PHAR" \
    --with-php-config=/usr/bin/oxphp-php-config \
    --with-phpize-path=/usr/bin/oxphp-phpize \
    "$@"
```

### oxphp-php (`staging/usr/bin/oxphp-php`)

Wrapper script (not the raw PHP binary) to set correct ini paths:

```bash
#!/bin/sh
export PHP_INI_SCAN_DIR="/etc/oxphp/8.4/server/conf.d"
exec /usr/lib/oxphp/bin/php -c /etc/oxphp/8.4/server/php.ini "$@"
```

The actual PHP CLI binary lives at `/usr/lib/oxphp/bin/php` (internal, not in PATH).

### oxphp-phpize / oxphp-php-config

Wrapper scripts that set correct paths:

**oxphp-phpize:**
```bash
#!/bin/sh
# phpize needs to find the build dir and headers
export PHP_AUTOCONF="${PHP_AUTOCONF:-autoconf}"
exec /usr/lib/oxphp/bin/phpize \
    --phpdir=/usr/lib/oxphp/20240924 \
    "$@"
```

**oxphp-php-config:**
Patched version of `php-config` with paths rewritten to OxPHP locations
during Dockerfile.pkg build (sed replace on the original php-config script).

### postinstall.sh

```bash
#!/bin/sh
set -e

ldconfig

if ! id www-data >/dev/null 2>&1; then
    useradd -r -s /usr/sbin/nologin -d /var/www www-data 2>/dev/null || true
fi

if [ "$1" = "configure" ] && [ -z "$2" ]; then
    # Fresh install (deb)
    chown -R www-data:www-data /var/www/html
    systemctl daemon-reload
    systemctl enable oxphp.service
    echo ""
    echo "OxPHP installed successfully."
    echo ""
    echo "  Server config:     /etc/oxphp/oxphp.env"
    echo "  PHP config:        /etc/oxphp/8.4/server/php.ini"
    echo "  PHP conf.d:        /etc/oxphp/8.4/server/conf.d/"
    echo "  Extensions:        /usr/lib/oxphp/20240924/"
    echo ""
    echo "  Install extension: sudo oxphp-pie install redis/phpredis"
    echo "  Start server:      sudo systemctl start oxphp"
    echo ""
elif [ "$1" = "1" ]; then
    # Fresh install (rpm)
    chown -R www-data:www-data /var/www/html
    systemctl daemon-reload
    systemctl enable oxphp.service
    # same message
else
    # Upgrade
    systemctl daemon-reload
    systemctl try-restart oxphp.service 2>/dev/null || true
fi
```

### preremove.sh

```bash
#!/bin/sh
set -e
if [ "$1" = "remove" ] || [ "$1" = "0" ]; then
    systemctl stop oxphp.service 2>/dev/null || true
    systemctl disable oxphp.service 2>/dev/null || true
fi
```

---

## 5. Dockerfile.pkg Changes

### Stage 4 (packager) — new steps

```dockerfile
# 1. Copy PHP CLI binary
COPY --from=builder /usr/local/bin/php staging/usr/lib/oxphp/bin/php

# 2. Copy phpize + php-config
COPY --from=builder /usr/local/bin/phpize staging/usr/lib/oxphp/bin/phpize
COPY --from=builder /usr/local/bin/php-config staging/usr/lib/oxphp/bin/php-config.orig

# 3. Rewrite php-config paths to oxphp layout
RUN sed \
    -e 's|/usr/local/include/php|/usr/include/oxphp/20240924|g' \
    -e 's|/usr/local/lib/php|/usr/lib/oxphp|g' \
    -e 's|/usr/local/etc/php|/etc/oxphp/8.4/server|g' \
    -e 's|/usr/local/bin/php|/usr/lib/oxphp/bin/php|g' \
    staging/usr/lib/oxphp/bin/php-config.orig > staging/usr/lib/oxphp/bin/php-config && \
    chmod 755 staging/usr/lib/oxphp/bin/php-config && \
    rm staging/usr/lib/oxphp/bin/php-config.orig

# 4. Copy PHP headers
COPY --from=builder /usr/local/include/php/ staging/usr/include/oxphp/20240924/

# 5. Copy phpize build files
COPY --from=builder /usr/local/lib/php/build/ staging/usr/lib/oxphp/20240924/build/

# 6. Copy extensions to versioned dir
RUN mkdir -p staging/usr/lib/oxphp/20240924
# opcache from builder
COPY --from=builder /usr/local/lib/php/extensions/ /tmp/php-ext/
RUN find /tmp/php-ext -name 'opcache.so' -exec cp {} staging/usr/lib/oxphp/20240924/ \; && \
    rm -rf /tmp/php-ext
# oxphp_sapi from ext-builder
COPY --from=ext-builder /usr/local/lib/php/extensions/ /tmp/ext/
RUN find /tmp/ext -name 'oxphp_sapi.so' -exec cp {} staging/usr/lib/oxphp/20240924/ \; && \
    rm -rf /tmp/ext

# 7. Copy php.ini-production
COPY --from=builder /usr/local/etc/php/php.ini-production \
    staging/etc/oxphp/8.4/server/php.ini

# 8. Generate extension.ini
RUN mkdir -p staging/etc/oxphp/8.4/server/conf.d && \
    printf 'extension_dir=/usr/lib/oxphp/20240924\nzend_extension=opcache.so\nextension=oxphp_sapi.so\n' \
    > staging/etc/oxphp/8.4/server/conf.d/extension.ini

# 9. Create wrapper scripts
RUN mkdir -p staging/usr/bin && \
    # oxphp-php wrapper
    printf '#!/bin/sh\nexport PHP_INI_SCAN_DIR="/etc/oxphp/8.4/server/conf.d"\nexec /usr/lib/oxphp/bin/php -c /etc/oxphp/8.4/server/php.ini "$@"\n' \
    > staging/usr/bin/oxphp-php && chmod 755 staging/usr/bin/oxphp-php && \
    # oxphp-phpize wrapper
    printf '#!/bin/sh\nexec /usr/lib/oxphp/bin/phpize "$@"\n' \
    > staging/usr/bin/oxphp-phpize && chmod 755 staging/usr/bin/oxphp-phpize && \
    # oxphp-php-config wrapper
    printf '#!/bin/sh\nexec /usr/lib/oxphp/bin/php-config "$@"\n' \
    > staging/usr/bin/oxphp-php-config && chmod 755 staging/usr/bin/oxphp-php-config
```

---

## 6. SAPI Code Change (src/php/sapi.rs)

Override compiled-in ini path so the OxPHP server process reads
`/etc/oxphp/8.4/server/php.ini`:

```rust
// In build_sapi_module():
php_ini_path_override: CString::new("/etc/oxphp/8.4/server")
    .unwrap().into_raw(),
```

And set the scan dir via environment before PHP startup:

```rust
std::env::set_var("PHP_INI_SCAN_DIR", "/etc/oxphp/8.4/server/conf.d");
```

This ensures both the server binary and CLI wrapper use the same config paths.

---

## 7. Systemd Service Updates

```ini
EnvironmentFile=-/etc/oxphp/oxphp.env
ReadOnlyPaths=/etc/oxphp /usr/lib/oxphp /usr/include/oxphp
```

---

## 8. extension.ini Contents

```ini
; Extension directory (vendored OxPHP ZTS extensions)
extension_dir=/usr/lib/oxphp/20240924

; Bundled extensions
zend_extension=opcache.so
extension=oxphp_sapi.so

; User extensions installed via oxphp-pie go here too.
; Example: oxphp-pie install redis/phpredis
; Creates /usr/lib/oxphp/20240924/redis.so and
; /etc/oxphp/8.4/server/conf.d/redis.ini
```

---

## 9. User Workflow

### Install OxPHP

```bash
sudo apt install ./oxphp_0.1.0_amd64.deb
# or from repo: sudo apt install oxphp
```

If system PHP is installed, apt shows removal plan and asks for confirmation.

### Install an extension

```bash
# First run downloads PIE automatically
sudo oxphp-pie install redis/phpredis

# PIE compiles ZTS-compatible .so → /usr/lib/oxphp/20240924/redis.so
# PIE creates ini → /etc/oxphp/8.4/server/conf.d/redis.ini

sudo systemctl restart oxphp
```

### Manage PIE version

```bash
# Update PIE to specific version
sudo oxphp-pie pie-update 1.4.0
```

### Use PHP CLI

```bash
oxphp-php -v          # PHP 8.4.x (ZTS)
oxphp-php script.php  # Run PHP script with OxPHP's ZTS runtime
```

---

## 10. Checklist

### Files to create

- [ ] `dist/oxphp-pie` — PIE wrapper script
- [ ] Wrapper scripts generated in Dockerfile.pkg (oxphp-php, oxphp-phpize, oxphp-php-config)

### Files to modify

- [ ] `nfpm.yaml` — new paths, conflicts/replaces/provides, recommends, all new contents
- [ ] `Dockerfile.pkg` — stage 4: copy CLI, phpize, php-config, headers, build files, wrappers
- [ ] `Dockerfile.bookworm-release` — align paths with new structure
- [ ] `dist/oxphp.service` — update ReadOnlyPaths
- [ ] `dist/oxphp.env` — no changes needed
- [ ] `dist/postinstall.sh` — updated install message
- [ ] `dist/preremove.sh` — no changes needed
- [ ] `src/php/sapi.rs` — set `php_ini_path_override` + `PHP_INI_SCAN_DIR`
- [ ] `.github/workflows/packages.yml` — smoke test may need update
- [ ] `.github/workflows/nightly.yml` — no changes needed

### Decisions (confirmed)

- [x] SAPI subdir name: `server`
- [x] PIE: download on first use (not bundled)
- [x] No `phpdbg` binary
- [x] No `phar` binary

### Future improvements

- [ ] Build PHP from source with custom `--with-config-file-path` to eliminate runtime overrides
- [ ] Separate `oxphp-dev` package (headers + phpize + build files) for smaller base package
- [ ] `oxphp-enmod` / `oxphp-dismod` scripts (like `phpenmod`/`phpdismod`)
