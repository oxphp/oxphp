#!/usr/bin/env bash
#
# gen-llms-txt.sh — generate llms.txt and llms-full.txt from docs.
#
# Walks docs/**/*.md, reads each file's frontmatter (title, description),
# groups files by their subdirectory into H2 sections, and writes two files to
# the project root:
#
#   llms.txt       — index of links, per the https://llmstxt.org spec
#   llms-full.txt  — same header plus the full markdown body of every page
#
# Links are relative to the project root by default (e.g. docs/features/tls.md).
# Set BASE_URL (env or --base-url) to prepend an absolute prefix and emit
# absolute links, e.g. BASE_URL=https://oxphp.dev/ -> https://oxphp.dev/docs/...
#
# Usage:
#   scripts/gen-llms-txt.sh [--base-url URL]
#   scripts/gen-llms-txt.sh --check
#
# --check regenerates into temp files and diffs them against the committed
# llms.txt / llms-full.txt, printing the drift and exiting non-zero instead of
# writing anything, so a docs change that forgets to refresh them fails the build.

set -euo pipefail

# --- locate project root (scripts/ lives directly under the root) -------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DOCS_REL="docs"
DOCS_DIR="$ROOT/$DOCS_REL"

# --- args --------------------------------------------------------------------
BASE_URL="${BASE_URL:-}"
CHECK=0
usage() { sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
  case "$1" in
    --base-url) BASE_URL="${2:-}"; shift 2 ;;
    --base-url=*) BASE_URL="${1#*=}"; shift ;;
    --check) CHECK=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "gen-llms-txt: unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$CHECK" = 1 ] && [ -n "$BASE_URL" ]; then
  echo "gen-llms-txt: --check compares against the committed relative-link files; drop --base-url" >&2
  exit 2
fi

[ -d "$DOCS_DIR" ] || { echo "gen-llms-txt: $DOCS_DIR not found" >&2; exit 1; }

# --- helpers -----------------------------------------------------------------

# read_meta FILE -> "title<TAB>description" from the leading YAML frontmatter.
# Surrounding double quotes are stripped; values are otherwise taken verbatim.
read_meta() {
  awk '
    NR==1 && $0 ~ /^---[[:space:]]*$/ { infm=1; next }
    infm && $0 ~ /^---[[:space:]]*$/  { exit }
    infm && $0 ~ /^title:[[:space:]]*/        { v=$0; sub(/^title:[[:space:]]*/,"",v); title=v }
    infm && $0 ~ /^description:[[:space:]]*/  { v=$0; sub(/^description:[[:space:]]*/,"",v); desc=v }
    END {
      gsub(/^"|"$/, "", title); gsub(/^"|"$/, "", desc)
      printf "%s\t%s\n", title, desc
    }
  ' "$1"
}

# strip_frontmatter FILE -> file body with the leading frontmatter block removed.
strip_frontmatter() {
  awk '
    NR==1 && $0 ~ /^---[[:space:]]*$/ { infm=1; next }
    infm && $0 ~ /^---[[:space:]]*$/  { infm=0; next }
    !infm { print }
  ' "$1"
}

# section_title DIRNAME -> human-readable H2 title.
section_title() {
  case "$1" in
    php) echo "PHP" ;;
    *)   echo "$1" | awk -F- '{ for (i=1;i<=NF;i++) $i=toupper(substr($i,1,1)) substr($i,2) }1' OFS=" " ;;
  esac
}

# gather DIR -> "title<TAB>description<TAB>relpath" per doc file, sorted by title.
gather() {
  local dir="$1" f base meta t d rel
  # Recurse so nested sections (e.g. examples/{framework,cms,ecommerce}/*.md) are
  # included; flat sections are unaffected.
  find "$dir" -type f -name '*.md' | while IFS= read -r f; do
    base="$(basename "$f")"
    case "$base" in CLAUDE.md|index.md) continue ;; esac
    meta="$(read_meta "$f")"
    t="${meta%%$'\t'*}"
    d="${meta#*$'\t'}"
    if [ -z "$t" ]; then
      t="$(grep -m1 '^# ' "$f" | sed 's/^# *//')"
    fi
    rel="${f#"$ROOT"/}"
    # Fields are joined with US (0x1f), a non-whitespace separator, so that an
    # empty description survives `read` (a tab would be IFS-collapsed away).
    printf '%s\037%s\037%s\n' "$t" "$d" "$rel"
  done | sort -f -t "$(printf '\037')" -k1,1
}

# --- section order (mirrors the docs nav) ------------------------------------
ordered="getting-started examples features shared-state security php operations architecture"
sections=""
for s in $ordered; do
  [ -d "$DOCS_DIR/$s" ] && sections="$sections $s"
done
for s in $(find "$DOCS_DIR" -mindepth 1 -maxdepth 1 -type d -exec basename {} \; | sort); do
  case " $ordered " in *" $s "*) ;; *) sections="$sections $s" ;; esac
done

# --- header ------------------------------------------------------------------
index_meta="$(read_meta "$DOCS_DIR/index.md" 2>/dev/null || true)"
summary="${index_meta#*$'\t'}"
[ -n "$summary" ] || summary="Documentation for OxPHP, a high-performance async PHP application server."

tmp_index="$(mktemp)"
tmp_full="$(mktemp)"
trap 'rm -f "$tmp_index" "$tmp_full"' EXIT

{ printf '# OxPHP\n\n> %s\n' "$summary"; } > "$tmp_index"
{ printf '# OxPHP\n\n> %s\n' "$summary"; } > "$tmp_full"

# --- body --------------------------------------------------------------------
count=0
for name in $sections; do
  title="$(section_title "$name")"
  printf '\n## %s\n\n' "$title" >> "$tmp_index"
  while IFS="$(printf '\037')" read -r t d rel; do
    [ -n "$t" ] || continue
    if [ -n "$BASE_URL" ]; then link="${BASE_URL%/}/$rel"; else link="$rel"; fi
    if [ -n "$d" ]; then
      printf -- '- [%s](%s): %s\n' "$t" "$link" "$d" >> "$tmp_index"
    else
      printf -- '- [%s](%s)\n' "$t" "$link" >> "$tmp_index"
    fi
    printf -- '\n---\n\n' >> "$tmp_full"
    strip_frontmatter "$ROOT/$rel" >> "$tmp_full"
    count=$((count + 1))
  done < <(gather "$DOCS_DIR/$name")
done

if [ "$CHECK" = 1 ]; then
  drift=0
  for name in llms.txt llms-full.txt; do
    case "$name" in
      llms.txt) generated="$tmp_index" ;;
      *)        generated="$tmp_full" ;;
    esac
    if [ ! -f "$ROOT/$name" ]; then
      echo "gen-llms-txt: $name is missing" >&2
      drift=1
      continue
    fi
    if ! diff -u --label "$name (committed)" --label "$name (regenerated)" \
         "$ROOT/$name" "$generated" >&2; then
      drift=1
    fi
  done
  if [ "$drift" = 1 ]; then
    echo "gen-llms-txt: generated files are stale — run scripts/gen-llms-txt.sh and commit the result" >&2
    exit 1
  fi
  echo "gen-llms-txt: llms.txt and llms-full.txt are up to date ($count pages)" >&2
  exit 0
fi

mv "$tmp_index" "$ROOT/llms.txt"
mv "$tmp_full" "$ROOT/llms-full.txt"
trap - EXIT

echo "gen-llms-txt: wrote llms.txt and llms-full.txt ($count pages, base-url='${BASE_URL:-<relative>}')" >&2
