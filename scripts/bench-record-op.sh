#!/usr/bin/env bash
# Runs the record_op overhead bench inside the OxPHP dev Docker image
# for production-like (Linux x86_64 musl) numbers. Forwards any extra
# arguments to `cargo bench` after `--`.
set -euo pipefail

echo "=== bench-record-op.sh ==="
echo "host: $(uname -srm)"
echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
if command -v sysctl >/dev/null 2>&1; then
    sysctl -n machdep.cpu.brand_string 2>/dev/null | sed 's/^/cpu : /' || true
elif [ -r /proc/cpuinfo ]; then
    grep -m1 '^model name' /proc/cpuinfo | sed 's/^/cpu : /'
fi

exec docker compose run --rm --no-deps oxphp \
    cargo bench --bench record_op_overhead \
    --no-default-features --features plugin-shared -- "$@"
