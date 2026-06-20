# OxPHP — Example Dockerfile

This is a **starting template** for building your own application image on top of the published OxPHP image. Copy both `Dockerfile` and `.dockerignore` into the root of your project and adapt them. The `.dockerignore` keeps your VCS history, `.env` secrets, and local artifacts out of the image, which the `COPY . /var/www/html` steps would otherwise pull in.

It is **not** the Dockerfile that builds the official `ghcr.io/oxphp/oxphp` image — that one lives in `docker/release/alpine/Dockerfile` and is for repository maintainers only.

## What it gives you

Three build targets in a single multi-stage Dockerfile:

| Target     | Purpose                                       | Contents                                                              |
|------------|-----------------------------------------------|-----------------------------------------------------------------------|
| `dev`      | Local development                             | OxPHP + PHP CLI + Composer + Xdebug, opcache validates timestamps     |
| `prod`     | Long-running production server                | Minimal runtime — only `oxphp`, `libphp.so`, your app code            |
| `prod-cli` | Short-lived CLI image (migrations, artisan)   | PHP CLI for `docker run --rm myapp:prod-cli php artisan migrate`      |

## Usage

```bash
docker build --target dev      -t myapp:dev      .
docker build --target prod     -t myapp:prod     .
docker build --target prod-cli -t myapp:prod-cli .
```

## Important: Alpine version must match

The Alpine version in your Dockerfile **must match** the one baked into the OxPHP image you pull (`ghcr.io/oxphp/oxphp:<tag>` ships on a specific `php:<ver>-zts-alpine<X.Y>`). A mismatch changes the musl version behind the precompiled `libphp.so` and the Rust binary, and can cause musl TLS corruption or undefined-symbol errors at startup.

Check which Alpine the tag you depend on uses, then pin `ARG ALPINE_VERSION` accordingly.
