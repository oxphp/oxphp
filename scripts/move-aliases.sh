#!/usr/bin/env bash
# scripts/move-aliases.sh
#
# Re-create all alias tags that should point at the canonical tag for the
# given (oxphp, PHP minor, distribution) triple, consulting
# .github/supported-versions.yml to decide which latest-oriented aliases
# apply.
#
# The distribution is the suffix of the canonical tag: `alpine<X.Y>` for the
# Alpine image, a Debian codename (`trixie`) for the Debian one. Alpine
# aliases name no distribution (`php8.5`, `latest`) and follow the latest
# Alpine listed. Debian aliases carry the codename (`php8.5-trixie`, and
# `trixie` for `latest`), so every codename keeps its own set moving.
#
# Usage:
#   scripts/move-aliases.sh <oxphp_version> <php_minor> <distro> [--dry-run|--list]
#
# Example:
#   scripts/move-aliases.sh 0.3.0 8.4 alpine3.23
#   scripts/move-aliases.sh 0.3.0 8.4 trixie --dry-run
#
# --dry-run prints each alias with the tag it would point at; --list prints
# only the alias names, one per line, for the signing step. Neither touches
# the registry.
#
# Environment variables:
#   REGISTRY        Registry prefix. Default: ghcr.io/oxphp/oxphp
#   CONFIG_FILE     Path to supported-versions.yml. Default: .github/supported-versions.yml

set -euo pipefail

OXPHP_VERSION="${1:?usage: $0 <oxphp_version> <php_minor> <distro> [--dry-run|--list]}"
PHP_MINOR="${2:?usage: $0 <oxphp_version> <php_minor> <distro> [--dry-run|--list]}"
DISTRO="${3:?usage: $0 <oxphp_version> <php_minor> <distro> [--dry-run|--list]}"
FLAG="${4:-}"

case "$FLAG" in
    ""|--dry-run|--list) ;;
    *)
        echo "error: unknown flag: $FLAG (expected --dry-run or --list)" >&2
        exit 2
        ;;
esac

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

CANONICAL="${REGISTRY}:${OXPHP_VERSION}-php${PHP_MINOR}-${DISTRO}"

# Semver-minor of oxphp version (e.g., "0.3.0" -> "0.3")
SEMVER_MINOR=$(echo "$OXPHP_VERSION" | cut -d. -f1,2)

if [[ "$DISTRO" == alpine* ]]; then
    SUFFIX=""
    LATEST_ALIAS="latest"
    MOVES_ALIASES=$([[ "$DISTRO" == "alpine${LATEST_ALPINE}" ]] && echo yes || echo no)
else
    SUFFIX="-${DISTRO}"
    LATEST_ALIAS="${DISTRO}"
    MOVES_ALIASES=yes
fi

move() {
    local alias="$1"
    if [[ "$FLAG" == "--list" ]]; then
        echo "${alias}"
    elif [[ "$FLAG" == "--dry-run" ]]; then
        echo "DRY RUN: ${alias} -> ${CANONICAL}"
    else
        echo "MOVE:    ${alias} -> ${CANONICAL}"
        docker buildx imagetools create -t "${alias}" "${CANONICAL}"
    fi
}

if [[ "$MOVES_ALIASES" == yes ]]; then
    # --- Version-scoped aliases ---

    # With a Debian codename this is the canonical tag itself.
    if [[ "${REGISTRY}:${OXPHP_VERSION}-php${PHP_MINOR}${SUFFIX}" != "$CANONICAL" ]]; then
        move "${REGISTRY}:${OXPHP_VERSION}-php${PHP_MINOR}${SUFFIX}"
    fi
    move "${REGISTRY}:${SEMVER_MINOR}-php${PHP_MINOR}${SUFFIX}"

    # --- Latest-oriented aliases (only moved if rebuilding the latest oxphp) ---

    if [[ "$OXPHP_VERSION" == "$LATEST_OXPHP" ]]; then
        move "${REGISTRY}:php${PHP_MINOR}${SUFFIX}"

        # Default-PHP aliases (only if this PHP is the project default)
        if [[ "$PHP_MINOR" == "$DEFAULT_PHP" ]]; then
            move "${REGISTRY}:${OXPHP_VERSION}${SUFFIX}"
            move "${REGISTRY}:${LATEST_ALIAS}"
        fi
    fi
fi

if [[ "$FLAG" != "--list" ]]; then
    echo "move-aliases.sh completed for ${OXPHP_VERSION} php${PHP_MINOR} ${DISTRO}"
fi
