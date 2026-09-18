#!/usr/bin/env bash
#
# gen-llms-txt.sh — generate llms.txt and llms-full.txt from docs.
#
# Walks the markdown under docs/ that git publishes — tracked files plus new
# ones that are not ignored — reads each file's frontmatter (title,
# description), groups files by their subdirectory into H2 sections, and writes
# two files to the project root:
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
#
# Sections come from each page's first path component, and a page named
# `index.md` is excluded — a page outside that set is not listed at all. A page
# inside it that yields no title, with no non-empty `title:` and no `# ` heading
# with text, has nothing to be listed under: both modes name it and exit
# non-zero without writing, because --check on its own would never report it —
# once such a run is committed, it recomputes the same omission, no drift.

set -euo pipefail

# --- locate project root (scripts/ lives directly under the root) -------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DOCS_REL="docs"
DOCS_DIR="$ROOT/$DOCS_REL"

# --- args --------------------------------------------------------------------
BASE_URL="${BASE_URL:-}"
CHECK=0
usage() { sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
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

# The answer, not the exit status: `rev-parse --is-inside-work-tree` exits 0 while
# printing `false` in a bare repository and under a GIT_DIR pointing at one, and
# this message is the whole point of the check — without it git's own
# "must be run in a work tree" is what the operator would get, from the
# `ls-files` call that builds the page list below.
[ "$(git -C "$ROOT" rev-parse --is-inside-work-tree 2>/dev/null)" = true ] || {
  echo "gen-llms-txt: needs a git checkout — the page list comes from git ls-files" >&2
  exit 1
}

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

# gather NAME -> "title<US>description<US>relpath" per page of section NAME,
# sorted by title. Nested sections (e.g. examples/{framework,cms,ecommerce}/*.md)
# come along on their own, because $pages is a flat list of paths.
gather() {
  local name="$1" f meta t d rel
  awk -v p="$DOCS_REL/$name/" 'index($0, p) == 1' "$pages" | while IFS= read -r rel; do
    f="$ROOT/$rel"
    # A path stays in the index after its working copy is deleted without
    # `git rm`. read_meta on a missing file prints an awk error and ends this
    # section early, dropping that page and every page after it in the section
    # while the run still writes its files and exits 0; `find` never listed such
    # a path in the first place.
    [ -f "$f" ] || {
      echo "gen-llms-txt: skipping $rel — git lists it, no regular file is there" >&2
      continue
    }
    meta="$(read_meta "$f")"
    t="${meta%%$'\t'*}"
    d="${meta#*$'\t'}"
    if [ -z "$t" ]; then
      # awk rather than `grep -m1 '^# ' | sed`: grep exits 1 on a file with no
      # H1, pipefail makes that the pipeline's status and therefore the
      # assignment's, and `set -e` then kills the subshell this loop runs in.
      # `sort` receives a truncated list, so the section loses this page and
      # every page after it, while the run writes its files and exits 0. awk
      # exits 0 whether or not it matched, so a page with no H1 no longer ends
      # the section. (awk does exit non-zero on a file it cannot open, but
      # read_meta above reads the same file first and would hit that already.)
      t="$(awk '/^# /{ sub(/^# */, ""); print; exit }' "$f")"
    fi
    # Fields are joined with US (0x1f), a non-whitespace separator, so that an
    # empty description survives `read` (a tab would be IFS-collapsed away).
    printf '%s\037%s\037%s\n' "$t" "$d" "$rel"
  done | sort -f -t "$(printf '\037')" -k1,1
}

# --- the pages git publishes -------------------------------------------------
# One source of truth for both the section list and each section's pages. A
# plain `find` over docs/ would instead take whatever the working copy happens
# to hold: a gitignored planning directory under docs/ became an H2 heading in
# llms.txt and pulled its whole body into llms-full.txt, and the only sign of it
# from this script was the page count it prints at the end. -c is the index, -o
# adds files not added to it yet, --exclude-standard drops the ignored ones from
# that second half (exclude patterns apply to --others and --ignored only, so a
# tracked file matching a .gitignore stays listed, which is right — git publishes
# it), and sort -u collapses the duplicate filenames the index reports for a path
# with unresolved merge stages.
#
# -z, then NUL to newline, because without it git wraps a path holding an
# "unusual" character in double quotes and escapes it the way C does: a double
# quote, a backslash or a control character always, and a byte above 0x80 unless
# core.quotePath is turned off. Such a line matches neither filter below, so the
# page would disappear from both artifacts with nothing on stderr and an exit
# status of 0, and --check would agree with itself forever, since CI recomputes
# the same omission from the same tree. A plain space is not "unusual" to git and
# needs none of this.
pages="$(mktemp)"
trap 'rm -f "$pages"' EXIT
git -C "$ROOT" ls-files -zco --exclude-standard -- "$DOCS_REL" \
  | tr '\0' '\n' \
  | awk -v root="$DOCS_REL/" '
      index($0, root) != 1 { next }
      $0 !~ /\.md$/        { next }
      { base = $0; sub(/.*\//, "", base) }
      base == "CLAUDE.md" || base == "index.md" { next }
      { print }
    ' \
  | sort -u > "$pages"

# Refuse to write rather than lay a file with nothing but the header over the
# committed one. Either the repository this runs in does not publish docs/ at all
# — it is on disk but ignored there, as when the tree is unpacked somewhere an
# enclosing .gitignore covers — or there is nothing under it to publish. Neither
# is a documentation tree worth overwriting the committed artifacts with.
[ -s "$pages" ] || {
  echo "gen-llms-txt: git lists no publishable markdown under $DOCS_REL/" >&2
  exit 1
}

# --- section order (mirrors the docs nav) ------------------------------------
# The sections are the first path component of the pages themselves, so a
# directory git does not publish never becomes a heading. Whether a section that
# is named here ends up with any pages is decided further down, by the loop that
# writes them.
present=" $(awk -v root="$DOCS_REL/" '
    { r = substr($0, length(root) + 1) }
    index(r, "/") { print substr(r, 1, index(r, "/") - 1) }
  ' "$pages" | sort -u | tr '\n' ' ')"
ordered="getting-started examples features shared-state security php operations architecture"
sections=""
for s in $ordered; do
  case "$present" in *" $s "*) sections="$sections $s" ;; esac
done
for s in $present; do
  case " $ordered " in *" $s "*) ;; *) sections="$sections $s" ;; esac
done

# --- header ------------------------------------------------------------------
index_meta="$(read_meta "$DOCS_DIR/index.md" 2>/dev/null || true)"
summary="${index_meta#*$'\t'}"
[ -n "$summary" ] || summary="Documentation for OxPHP, a high-performance async PHP application server."

tmp_index="$(mktemp)"
tmp_full="$(mktemp)"
trap 'rm -f "$pages" "$tmp_index" "$tmp_full"' EXIT

{ printf '# OxPHP\n\n> %s\n' "$summary"; } > "$tmp_index"
{ printf '# OxPHP\n\n> %s\n' "$summary"; } > "$tmp_full"

# --- body --------------------------------------------------------------------
count=0
dropped=0
for name in $sections; do
  title="$(section_title "$name")"
  # The heading is written by the first page that makes it, not by the section
  # existing. A section can still come out empty after its name is known — a
  # path the index lists whose working copy was deleted without `git rm` is
  # skipped below — and a heading with nothing under it is exactly the kind of
  # content that has no business in a published file. A page with neither a
  # title nor an H1 is skipped there too, but that one ends the run rather than
  # reaching a file. The loop body runs in this shell (the page list arrives by
  # process substitution, not a pipe), so what it sets survives the loop — this
  # flag and the dropped counter both depend on that.
  emitted=0
  while IFS="$(printf '\037')" read -r t d rel; do
    [ -n "$t" ] || {
      echo "gen-llms-txt: $rel yields no title — no non-empty 'title:' in its frontmatter, no '# ' heading with text" >&2
      dropped=$((dropped + 1))
      continue
    }
    [ "$emitted" = 1 ] || { printf '\n## %s\n\n' "$title" >> "$tmp_index"; emitted=1; }
    if [ -n "$BASE_URL" ]; then link="${BASE_URL%/}/$rel"; else link="$rel"; fi
    if [ -n "$d" ]; then
      printf -- '- [%s](%s): %s\n' "$t" "$link" "$d" >> "$tmp_index"
    else
      printf -- '- [%s](%s)\n' "$t" "$link" >> "$tmp_index"
    fi
    printf -- '\n---\n\n' >> "$tmp_full"
    strip_frontmatter "$ROOT/$rel" >> "$tmp_full"
    count=$((count + 1))
  done < <(gather "$name")
done

# A page that yields no title reaches neither artifact, and neither mode notices
# on its own: once such a run is committed, the generated file and the committed
# one omit the page alike, so --check diffs them clean and answers "up to date"
# for as long as the page stays untitled. Refuse instead — in both modes, and
# before anything is written, so the committed artifacts survive the refusal
# intact. This counts only pages that reached the loop above: a page directly
# under docs/ belongs to no section, so it never gets that far.
[ "$dropped" = 0 ] || {
  echo "gen-llms-txt: $dropped page(s) named above would be dropped — give each one a non-empty 'title:' or a '# ' heading with text" >&2
  exit 1
}

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
rm -f "$pages"
trap - EXIT

echo "gen-llms-txt: wrote llms.txt and llms-full.txt ($count pages, base-url='${BASE_URL:-<relative>}')" >&2
