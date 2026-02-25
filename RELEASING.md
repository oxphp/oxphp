# Releasing OxPHP

## Version sources

| Artifact | Version source |
|----------|---------------|
| Rust binary | `Cargo.toml` → `package.version` |
| .deb / .rpm | Git tag → `--build-arg VERSION=` → nfpm |
| Docker images | Git tag → manifest tags |

## Release checklist

```bash
# 1. Update version in Cargo.toml
#    version = "0.2.0"

# 2. Commit
git commit -am "Release v0.2.0"

# 3. Tag and push
git tag v0.2.0
git push origin main --tags
```

CI will automatically:
- Build Docker images (Alpine + Bookworm) tagged `0.2.0`, `latest`, `0.2.0-bookworm`, `bookworm`
- Build .deb and .rpm packages for amd64 and arm64
- Attach packages to the GitHub Release

## What happens on each push

### Push to `main` (no tag)

| Artifact | Tags / names |
|----------|-------------|
| Docker Alpine | `nightly`, `sha-{commit}` |
| Docker Bookworm | `nightly-bookworm`, `bookworm-sha-{commit}` |
| .deb / .rpm | `0.0.0-dev.{short-sha}` — uploaded as GitHub Actions artifacts (30 days) |

### Push tag `v*`

| Artifact | Tags / names |
|----------|-------------|
| Docker Alpine | `{version}`, `latest` |
| Docker Bookworm | `{version}-bookworm`, `bookworm` |
| .deb / .rpm | `{version}` — attached to GitHub Release |

## CI workflows

| File | Trigger | What it does |
|------|---------|-------------|
| `.github/workflows/nightly.yml` | push to `main` or `v*` tag | Builds Alpine + Bookworm Docker images (amd64/arm64), creates multi-arch manifests |
| `.github/workflows/packages.yml` | push to `main` or `v*` tag | Builds .deb + .rpm via `Dockerfile.pkg` + nfpm, attaches to Release on tag |

## Docker image tags

```
ghcr.io/oxphp/oxphp:latest              # latest release, Alpine
ghcr.io/oxphp/oxphp:0.2.0               # specific release, Alpine
ghcr.io/oxphp/oxphp:nightly             # latest main, Alpine
ghcr.io/oxphp/oxphp:sha-abc1234         # specific commit, Alpine

ghcr.io/oxphp/oxphp:bookworm            # latest release, Debian
ghcr.io/oxphp/oxphp:0.2.0-bookworm      # specific release, Debian
ghcr.io/oxphp/oxphp:nightly-bookworm    # latest main, Debian
ghcr.io/oxphp/oxphp:bookworm-sha-abc123 # specific commit, Debian
```

## Package installation

```bash
# Debian / Ubuntu
sudo apt install ./oxphp_0.2.0-1_amd64.deb

# RHEL / Fedora
sudo rpm -i oxphp-0.2.0-1.x86_64.rpm

# Configure and start
sudo vim /etc/oxphp/oxphp.env
sudo systemctl start oxphp
```

## Package contents

```
/usr/bin/oxphp                          # server binary
/usr/lib/oxphp/libphp.so                # vendored PHP 8.4 ZTS runtime
/usr/lib/oxphp/liboxphp_bridge.so       # C bridge library
/usr/lib/oxphp/extensions/opcache.so    # OPcache
/usr/lib/oxphp/extensions/oxphp_sapi.so # PHP extension
/etc/oxphp/php.ini                      # PHP config (preserved on upgrade)
/etc/oxphp/oxphp.env                    # env config (preserved on upgrade)
/etc/oxphp/extension.ini                # extension loading
/etc/ld.so.conf.d/oxphp.conf            # linker search path
/lib/systemd/system/oxphp.service       # systemd unit
/var/www/html/public/                   # web root
```
