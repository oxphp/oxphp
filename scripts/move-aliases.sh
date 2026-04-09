#!/usr/bin/env bash
# scripts/move-aliases.sh
#
# Re-create all alias tags that should point at the canonical tag for the
# given (oxphp, PHP minor, Alpine) triple, consulting
# .github/supported-versions.yml to decide which latest-oriented aliases
# apply.
#
# Usage:
#   scripts/move-aliases.sh <oxphp_version> <php_minor> <alpine_version> [--dry-run]
#
# Example:
#   scripts/move-aliases.sh 0.3.0 8.4 3.23
#   scripts/move-aliases.sh 0.3.0 8.4 3.23 --dry-run
#
# Environment variables:
#   REGISTRY        Registry prefix. Default: ghcr.io/oxphp/oxphp
#   CONFIG_FILE     Path to supported-versions.yml. Default: .github/supported-versions.yml

set -euo pipefail

OXPHP_VERSION="${1:?usage: $0 <oxphp_version> <php_minor> <alpine_version> [--dry-run]}"
PHP_MINOR="${2:?usage: $0 <oxphp_version> <php_minor> <alpine_version> [--dry-run]}"
ALPINE_VER="${3:?usage: $0 <oxphp_version> <php_minor> <alpine_version> [--dry-run]}"
FLAG="${4:-}"

REGISTRY="${REGISTRY:-ghcr.io/oxphp/oxphp}"
CONFIG_FILE="${CONFIG_FILE:-.github/supported-versions.yml}"

if [[ ! -f "$CONFIG_FILE" ]]; then
    echo "error: config file not found: $CONFIG_FILE" >&2
    exit 2
fi

if ! command -v yq >/dev/null 2>&1; then
    echo "error: yq is required (see https://github.com/mikefarah/yq)" >&2
    exit 2
fi

# Read config
LATEST_OXPHP=$(yq '.oxphp_versions[0]' "$CONFIG_FILE")
DEFAULT_PHP=$(yq '.default_php' "$CONFIG_FILE")
LATEST_ALPINE=$(yq '.alpine_versions[0]' "$CONFIG_FILE")

CANONICAL="${REGISTRY}:${OXPHP_VERSION}-php${PHP_MINOR}-alpine${ALPINE_VER}"

# Semver-minor of oxphp version (e.g., "0.3.0" -> "0.3")
SEMVER_MINOR=$(echo "$OXPHP_VERSION" | cut -d. -f1,2)

move() {
    local alias="$1"
    if [[ "$FLAG" == "--dry-run" ]]; then
        echo "DRY RUN: ${alias} -> ${CANONICAL}"
    else
        echo "MOVE:    ${alias} -> ${CANONICAL}"
        docker buildx imagetools create -t "${alias}" "${CANONICAL}"
    fi
}

# --- Version-scoped aliases (only moved if this run targets the latest Alpine) ---

if [[ "$ALPINE_VER" == "$LATEST_ALPINE" ]]; then
    move "${REGISTRY}:${OXPHP_VERSION}-php${PHP_MINOR}"
    move "${REGISTRY}:${SEMVER_MINOR}-php${PHP_MINOR}"
fi

# --- Latest-oriented aliases (only moved if rebuilding the latest oxphp) ---

if [[ "$OXPHP_VERSION" == "$LATEST_OXPHP" && "$ALPINE_VER" == "$LATEST_ALPINE" ]]; then
    move "${REGISTRY}:php${PHP_MINOR}"

    # Default-PHP aliases (only if this PHP is the project default)
    if [[ "$PHP_MINOR" == "$DEFAULT_PHP" ]]; then
        move "${REGISTRY}:${OXPHP_VERSION}"
        move "${REGISTRY}:latest"
    fi
fi

echo "move-aliases.sh completed for ${OXPHP_VERSION} php${PHP_MINOR} alpine${ALPINE_VER}"
