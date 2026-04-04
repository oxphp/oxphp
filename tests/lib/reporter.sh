#!/usr/bin/env bash
# Report generation: terminal, JSON, HTML.
# Sourced by run_all.sh.

# report_terminal <jsonl_file> [verbose]
report_terminal() {
    local jsonl_file="$1"
    local verbose="${2:-}"

    python3 - "$jsonl_file" "$verbose" << 'PYEOF'
import sys, json

jsonl_file = sys.argv[1]
verbose = len(sys.argv) > 2 and sys.argv[2] == "--verbose"

RED = "\033[0;31m"
GREEN = "\033[0;32m"
YELLOW = "\033[0;33m"
BOLD = "\033[1m"
RESET = "\033[0m"

passed = failed = errors = 0
output_lines = []

with open(jsonl_file) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            t = json.loads(line)
        except json.JSONDecodeError:
            continue

        test_name = t.get("test", "")
        group = t.get("group", "")
        profile = t.get("profile", "?")
        error = t.get("error") or ""
        is_pass = t.get("pass", False)
        label = f"{group}/{test_name}"
        pad = max(1, 55 - len(label) - len(profile))
        dots = "." * pad

        if error:
            errors += 1
            output_lines.append(f"  {RED}[{profile:<10}]{RESET} {label} {dots} {RED}ERROR{RESET}")
            output_lines.append(f"    {RED}{error}{RESET}")
        elif is_pass:
            passed += 1
            output_lines.append(f"  {GREEN}[{profile:<10}]{RESET} {label} {dots} {GREEN}PASS{RESET}")
            if verbose:
                for a in t.get("assertions", []):
                    mark = "✓" if a["pass"] else "✗"
                    output_lines.append(f"    {mark} {a['name']}")
        else:
            failed += 1
            output_lines.append(f"  {RED}[{profile:<10}]{RESET} {label} {dots} {RED}FAIL{RESET}")
            for a in t.get("assertions", []):
                if not a["pass"]:
                    exp = a.get("expected", "")
                    act = a.get("actual", "")
                    output_lines.append(f"    ✗ {a['name']} — expected: {exp}, actual: {act}")

total = passed + failed + errors
for line in output_lines:
    print(line)
print()
print(f"  {BOLD}Results:{RESET} {GREEN}{passed} passed{RESET}, {RED}{failed} failed{RESET}, {YELLOW}{errors} errors{RESET} ({total} total)")

sys.exit(0 if failed == 0 and errors == 0 else 1)
PYEOF
}

# report_json <jsonl_file> <output_file> <duration_seconds>
report_json() {
    local jsonl_file="$1"
    local output_file="$2"
    local duration="$3"

    python3 - "$jsonl_file" "$output_file" "$duration" << 'PYEOF'
import sys, json
from datetime import datetime, timezone

jsonl_file, output_file, duration = sys.argv[1], sys.argv[2], float(sys.argv[3])

tests = []
with open(jsonl_file) as f:
    for line in f:
        line = line.strip()
        if line:
            try:
                tests.append(json.loads(line))
            except json.JSONDecodeError:
                pass

passed = sum(1 for t in tests if t.get("pass"))
failed = sum(1 for t in tests if not t.get("pass") and not t.get("error"))
errors_count = sum(1 for t in tests if t.get("error"))

profiles = {}
for t in tests:
    p = t.get("profile", "unknown")
    g = t.get("group", "unknown")
    profiles.setdefault(p, {"suites": {}})
    profiles[p]["suites"].setdefault(g, {"tests": []})
    profiles[p]["suites"][g]["tests"].append(t)

report = {
    "timestamp": datetime.now(timezone.utc).isoformat(),
    "duration_seconds": duration,
    "summary": {"total": len(tests), "passed": passed, "failed": failed, "errors": errors_count},
    "profiles": profiles,
}

with open(output_file, "w") as f:
    json.dump(report, f, indent=2, ensure_ascii=False)
PYEOF
}

# report_html <jsonl_file> <output_file> <duration_seconds>
report_html() {
    local jsonl_file="$1"
    local output_file="$2"
    local duration="$3"

    python3 - "$jsonl_file" "$output_file" "$duration" << 'PYEOF'
import sys, json, html as html_mod
from datetime import datetime, timezone

jsonl_file, output_file, duration = sys.argv[1], sys.argv[2], float(sys.argv[3])

tests = []
with open(jsonl_file) as f:
    for line in f:
        line = line.strip()
        if line:
            try:
                tests.append(json.loads(line))
            except json.JSONDecodeError:
                pass

passed = sum(1 for t in tests if t.get("pass"))
failed = sum(1 for t in tests if not t.get("pass") and not t.get("error"))
errors_count = sum(1 for t in tests if t.get("error"))
total = len(tests)
ts = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")

h = lambda s: html_mod.escape(str(s))

html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>OxPHP Test Report</title>
<style>
* {{ margin:0; padding:0; box-sizing:border-box; }}
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background:#1a1a2e; color:#eee; padding:20px; }}
.summary {{ display:flex; gap:20px; margin-bottom:20px; flex-wrap:wrap; }}
.summary .card {{ padding:15px 25px; border-radius:8px; font-size:1.2em; font-weight:bold; }}
.card.pass {{ background:#16a34a; }} .card.fail {{ background:#dc2626; }} .card.error {{ background:#d97706; }} .card.total {{ background:#2563eb; }}
.meta {{ color:#888; margin-bottom:20px; font-size:0.9em; }}
details {{ margin-bottom:4px; }}
summary {{ cursor:pointer; padding:6px 10px; border-radius:4px; font-family:monospace; font-size:0.95em; }}
summary:hover {{ background:#2a2a4a; }}
.pass-label {{ color:#4ade80; }} .fail-label {{ color:#f87171; }} .error-label {{ color:#fbbf24; }}
.assertions {{ margin-left:30px; padding:8px 0; font-size:0.9em; }}
.assertions .ok {{ color:#4ade80; }} .assertions .ko {{ color:#f87171; }}
.filter {{ margin-bottom:15px; }}
.filter button {{ padding:6px 14px; border:1px solid #444; background:transparent; color:#eee; border-radius:4px; cursor:pointer; margin-right:6px; }}
.filter button.active {{ background:#2563eb; border-color:#2563eb; }}
.profile-section {{ margin-bottom:20px; }}
.profile-title {{ font-size:1.1em; font-weight:bold; color:#93c5fd; padding:8px 0; border-bottom:1px solid #333; margin-bottom:8px; }}
</style>
</head>
<body>
<h1>OxPHP Test Report</h1>
<div class="meta">{h(ts)} &middot; {duration:.1f}s</div>
<div class="summary">
  <div class="card total">{total} total</div>
  <div class="card pass">{passed} passed</div>
  <div class="card fail">{failed} failed</div>
  <div class="card error">{errors_count} errors</div>
</div>
<div class="filter">
  <button class="active" onclick="filterTests('all',this)">All</button>
  <button onclick="filterTests('fail',this)">Failed</button>
  <button onclick="filterTests('error',this)">Errors</button>
</div>
<div id="results">
"""

profiles = {}
for t in tests:
    p = t.get("profile", "unknown")
    profiles.setdefault(p, []).append(t)

for profile, ptests in sorted(profiles.items()):
    html += f'<div class="profile-section">'
    html += f'<div class="profile-title">[{h(profile)}]</div>'
    for t in ptests:
        tname = h(t.get("test", ""))
        group = h(t.get("group", ""))
        error = t.get("error") or ""
        is_pass = t.get("pass", False)
        label = f"{group}/{tname}"
        cls = "error" if error else ("pass" if is_pass else "fail")
        status_html = f'<span class="{cls}-label">{"ERROR" if error else ("PASS" if is_pass else "FAIL")}</span>'
        is_open = "open" if cls != "pass" else ""
        html += f'<details {is_open} data-status="{cls}"><summary>{status_html} {label}</summary>'
        html += '<div class="assertions">'
        if error:
            html += f'<div class="ko">{h(error)}</div>'
        for a in t.get("assertions", []):
            aname = h(a.get("name", ""))
            if a.get("pass"):
                html += f'<div class="ok">✓ {aname}</div>'
            else:
                exp = h(a.get("expected", ""))
                act = h(a.get("actual", ""))
                html += f'<div class="ko">✗ {aname} — expected: {exp}, actual: {act}</div>'
        html += '</div></details>'
    html += '</div>'

html += """</div>
<script>
function filterTests(status, btn) {
  document.querySelectorAll('.filter button').forEach(b => b.classList.remove('active'));
  btn.classList.add('active');
  document.querySelectorAll('details').forEach(d => {
    if (status === 'all') d.style.display = '';
    else d.style.display = d.dataset.status === status ? '' : 'none';
  });
}
</script>
</body></html>"""

with open(output_file, "w") as f:
    f.write(html)
PYEOF
}
