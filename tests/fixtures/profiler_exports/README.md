# Profiler exporter golden fixtures

Reference outputs for the `tiny` 3-span tree (outer → middle → inner)
hand-constructed in `tests/profiler_export_fixtures_tests.rs::make_fixture_tree`
with deterministic timestamps, `cpu_ns`, and memory values. Use these
to detect unintended wire-format changes during refactors.

## Files

| File | Source | Compare |
|------|--------|---------|
| `tiny.collapsed`           | `export_collapsed(tree, Wall)` | byte-equal |
| `tiny.collapsed.cpu`       | `export_collapsed(tree, Cpu)`  | byte-equal |
| `tiny.collapsed.mem`       | `export_collapsed(tree, Mem)`  | byte-equal (may be empty) |
| `tiny.xhprof.json`         | `export_xhprof(tree, Raw, None)` | semantic JSON-equal |
| `tiny.xhprof.xhgui.json`   | `export_xhprof(tree, Xhgui, Some(meta))` | semantic JSON-equal |
| `tiny.speedscope.json`     | `export_speedscope(tree)` | semantic JSON-equal |

(pprof has no committed fixture — gzip + protobuf field-ordering
make byte equality fragile. The test decodes the bytes back into a
`Profile` struct and compares structurally.)

## Regenerating

When intentionally changing a format:

```bash
cargo test --features plugin-profiler --no-default-features \
  --test profiler_export_fixtures_tests \
  regenerate_fixtures -- --ignored --nocapture
```

Eyeball the diff (`git diff tests/fixtures/profiler_exports/`), then
commit the new fixtures alongside the format change with a clear
"intentional format change because …" justification in the commit
message.
