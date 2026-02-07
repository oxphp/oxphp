# Build stage
FROM rust:1.85-alpine AS builder

# Install build dependencies
RUN apk add --no-cache musl-dev

WORKDIR /build

# Copy dependency files first (layer caching)
COPY Cargo.toml Cargo.lock ./

# Create dummy source to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    touch src/lib.rs && \
    cargo build --release && \
    rm -rf src target/release/oxphp target/release/deps/oxphp-* target/release/.fingerprint/oxphp-*

# Copy real source
COPY src ./src

# Build release binary
RUN cargo build --release

# Runtime stage
FROM alpine:3.21

# Install runtime dependencies
RUN apk add --no-cache libgcc

# Create www-data user (UID 82, compatible with nginx/apache)
# Alpine 3.21 already has www-data group (GID 82), so add user to existing group
RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true

# Create web root
RUN mkdir -p /var/www/html && chown www-data:www-data /var/www/html

# Copy binary from builder
COPY --from=builder /build/target/release/oxphp /usr/local/bin/oxphp

# Copy default web files
COPY --chown=www-data:www-data www/ /var/www/html/

USER www-data

EXPOSE 8080

CMD ["oxphp"]
