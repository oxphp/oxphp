#!/usr/bin/env bash
#
# check-links.sh — verify every in-repo link in the tracked markdown resolves.
#
# Walks the tracked *.md files plus the @link annotations in oxphp.stub.php and
# checks two things per link:
#
#   * the target path exists and is tracked by git — a link to a file that is
#     present only in a working copy is broken for everyone who clones
#   * a #fragment matches a heading in the target file, using GitHub's slug
#     rules (lowercase, punctuation dropped, spaces to hyphens, -1/-2 suffixes
#     for repeated headings) or an explicit <a name>/<a id> anchor
#
# Fenced code blocks are skipped so sample commands and configuration are not
# mistaken for links. http(s), mailto and tel targets are counted and left
# alone: liveness needs the network and does not belong in a per-push gate.
#
# Usage:
#   scripts/check-links.sh          # exits non-zero and lists every broken link

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

paths="$(mktemp)"
sources="$(mktemp)"
trap 'rm -f "$paths" "$sources"' EXIT

# Every tracked path, plus each directory prefix, so a link to a directory
# (examples/dockerfile/) resolves the same way a link to a file does.
git ls-files -z | tr '\0' '\n' | awk '
  { print $0
    p = $0
    while (match(p, "/[^/]*$")) { p = substr(p, 1, RSTART - 1); print p "/" ; print p }
  }
' | sort -u > "$paths"

git ls-files -z '*.md' | tr '\0' '\n' > "$sources"

# The PHP stub carries @link annotations pointing at documentation pages. They
# are plain paths from the project root, so they are checked separately from
# the markdown scan below.
stub_broken=0
if [ -f oxphp.stub.php ]; then
  while read -r target; do
    [ -n "$target" ] || continue
    case "$target" in http://*|https://*) continue ;; esac
    file="${target%%#*}"
    if ! grep -qxF "$file" "$paths"; then
      echo "oxphp.stub.php: @link $target -> missing $file"
      stub_broken=$((stub_broken + 1))
    fi
  done < <(grep -o '@link[[:space:]]\+[^[:space:]]\+' oxphp.stub.php | awk '{print $2}')
fi

md_report="$(mktemp)"
trap 'rm -f "$paths" "$sources" "$md_report"' EXIT

# Pass the file list as positional arguments so a name with a space stays one
# argument; awk cannot read the list itself and also read the files.
set --
while IFS= read -r f; do [ -n "$f" ] && set -- "$@" "$f"; done < "$sources"

awk -v pathlist="$paths" '
function slug(s,   i, out) {
  gsub(/<[^>]*>/, "", s)                       # inline html
  while (match(s, /!?\[[^]]*\]\([^)]*\)/)) {   # [text](url) -> text
    out = substr(s, RSTART, RLENGTH)
    sub(/^!?\[/, "", out); sub(/\].*$/, "", out)
    s = substr(s, 1, RSTART - 1) out substr(s, RSTART + RLENGTH)
  }
  gsub(/[^a-zA-Z0-9 \t_-]/, "", s)             # GitHub keeps only these
  sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s)
  s = tolower(s)
  gsub(/[ \t]/, "-", s)                        # one hyphen per space, runs are not collapsed
  return s
}
function dirname(p) { return (p ~ /\//) ? substr(p, 1, length(p) - length(basename(p)) - 1) : "." }
function basename(p,   n) { n = p; sub(/^.*\//, "", n); return n }
function normalize(p,   parts, n, i, out, k) {
  n = split(p, parts, "/")
  k = 0
  for (i = 1; i <= n; i++) {
    if (parts[i] == "" || parts[i] == ".") continue
    if (parts[i] == "..") { if (k > 0) k--; else return "" ; continue }
    out[++k] = parts[i]
  }
  p = ""
  for (i = 1; i <= k; i++) p = (i == 1) ? out[i] : p "/" out[i]
  return p
}
BEGIN { while ((getline line < pathlist) > 0) exists[line] = 1 }

FNR == 1 { fence = 0 }
/^[ \t]*(```|~~~)/ { fence = !fence; next }
fence { next }
{
  line = $0

  # headings -> anchors, with GitHub'"'"'s -1/-2 suffix for repeats
  if (match(line, /^#+[ \t]+/)) {
    title = substr(line, RSTART + RLENGTH)
    sub(/[ \t]*#+[ \t]*$/, "", title)
    s = slug(title)
    if (s != "") {
      key = FILENAME SUBSEP s
      if (key in anchor) { anchor[FILENAME SUBSEP s "-" seen[key]] = 1; seen[key]++ }
      else { anchor[key] = 1; seen[key] = 1 }
    }
  }
  while (match(line, /<a[ \t][^>]*(name|id)[ \t]*=[ \t]*"[^"]*"/)) {
    frag = substr(line, RSTART, RLENGTH)
    sub(/^.*=[ \t]*"/, "", frag); sub(/".*$/, "", frag)
    anchor[FILENAME SUBSEP frag] = 1
    line = substr(line, RSTART + RLENGTH)
  }

  # links: inline [text](target) and reference definitions [id]: target
  line = $0
  while (match(line, /\]\([^)]*\)/)) {
    t = substr(line, RSTART + 2, RLENGTH - 3)
    line = substr(line, RSTART + RLENGTH)
    record(t)
  }
  if (match($0, /^[ \t]*\[[^]]+\][ \t]*:[ \t]*[^ \t]+/)) {
    t = substr($0, RSTART, RLENGTH)
    sub(/^[ \t]*\[[^]]*\][ \t]*:[ \t]*/, "", t)   # only the [id]: prefix, not every colon
    record(t)
  }
}
function record(t) {
  sub(/^[ \t]*<?/, "", t); sub(/>?[ \t]*$/, "", t)
  sub(/[ \t]+".*$/, "", t)          # [text](path "title")
  if (t == "") return
  n_links++
  links_file[n_links] = FILENAME
  links_line[n_links] = FNR
  links_target[n_links] = t
}
END {
  for (i = 1; i <= n_links; i++) {
    t = links_target[i]
    if (t ~ /^(https?:|mailto:|tel:)/) { external++; continue }
    src = links_file[i]
    if (t ~ /^#/) {
      frag = substr(t, 2)
      if (frag != "" && !((src SUBSEP frag) in anchor))
        printf "%s:%d: %s -> no heading in this file\n", src, links_line[i], t
      continue
    }
    split(t, part, "#")
    file = part[1]; frag = (index(t, "#") ? substr(t, index(t, "#") + 1) : "")
    target = (substr(file, 1, 1) == "/") ? normalize(substr(file, 2)) \
                                         : normalize(dirname(src) "/" file)
    if (target == "" || !(target in exists)) {
      printf "%s:%d: %s -> missing %s\n", src, links_line[i], t, (target == "" ? file : target)
      continue
    }
    if (frag != "" && target ~ /\.md$/ && !((target SUBSEP frag) in anchor))
      printf "%s:%d: %s -> no heading \"%s\" in %s\n", src, links_line[i], t, frag, target
  }
  printf "checked %d in-repo links (%d external, not fetched)\n", n_links - external, external > "/dev/stderr"
}
' "$@" > "$md_report"

md_broken=$(grep -c . "$md_report" || true)
cat "$md_report"

total=$((md_broken + stub_broken))
if [ "$total" -gt 0 ]; then
  echo "check-links: $total broken link(s)" >&2
  exit 1
fi
echo "check-links: no broken links" >&2
