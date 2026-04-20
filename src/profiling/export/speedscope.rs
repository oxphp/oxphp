//! Speedscope evented-profile exporter (Speedscope file-format v1).
//!
//! Output is a JSON document that Speedscope (https://speedscope.app)
//! imports directly. The "evented" profile type carries one `O`/`C`
//! event per BEGIN/END, with frames interned in a shared table.
//!
//! The exporter walks the tree's finished spans, collects unique
//! function names into a frames table, and emits open/close events
//! in chronological order. The earliest `start_ns` becomes the
//! profile's t=0; the latest `end_ns` becomes its end value. Units
//! are microseconds throughout.

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::profiling::SpanTree;

/// Render the tree as a Speedscope JSON profile. Returns UTF-8
/// bytes (compact JSON, no trailing newline).
///
/// An empty tree yields a minimal valid document with an empty
/// events array and zero-length time window.
pub fn export_speedscope(tree: &SpanTree) -> Vec<u8> {
    // Profile time window: t=0 at the earliest start_ns; endValue at
    // the latest end_ns. Both expressed in µs relative to start.
    let (profile_start_ns, profile_end_ns) = if tree.finished.is_empty() {
        (0u64, 0u64)
    } else {
        let mut min_start = u64::MAX;
        let mut max_end = 0u64;
        for s in &tree.finished {
            if s.start_ns < min_start {
                min_start = s.start_ns;
            }
            if s.end_ns > max_end {
                max_end = s.end_ns;
            }
        }
        (min_start, max_end)
    };
    let end_us = profile_end_ns.saturating_sub(profile_start_ns) / 1000;

    // Build the frames table. Insertion order is the index order;
    // Speedscope events reference frames by index.
    let mut frame_index: HashMap<&str, u32> = HashMap::new();
    let mut frames: Vec<&str> = Vec::with_capacity(tree.finished.len());
    for span in &tree.finished {
        frame_index.entry(span.name.as_ref()).or_insert_with(|| {
            let idx = frames.len() as u32;
            frames.push(span.name.as_ref());
            idx
        });
    }

    // Collect all O/C events with absolute µs timestamps.
    let mut events: Vec<(u64, &'static str, u32)> = Vec::with_capacity(tree.finished.len() * 2);
    for span in &tree.finished {
        let frame = *frame_index.get(span.name.as_ref()).expect("interned");
        let open_us = span.start_ns.saturating_sub(profile_start_ns) / 1000;
        let close_us = span.end_ns.saturating_sub(profile_start_ns) / 1000;
        events.push((open_us, "O", frame));
        events.push((close_us, "C", frame));
    }

    // Sort by (timestamp, kind, frame) where closes precede opens at
    // identical timestamps — proper LIFO discipline when an inner
    // span closes at the same µs an outer sibling opens.
    events.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| {
                let ord_a = if a.1 == "C" { 0 } else { 1 };
                let ord_b = if b.1 == "C" { 0 } else { 1 };
                ord_a.cmp(&ord_b)
            })
            .then_with(|| a.2.cmp(&b.2))
    });

    let event_array: Vec<Value> = events
        .into_iter()
        .map(|(at, kind, frame)| json!({"type": kind, "at": at, "frame": frame}))
        .collect();

    let frame_array: Vec<Value> = frames
        .into_iter()
        .map(|name| json!({"name": name}))
        .collect();

    let profile_name = if tree.trace_id.is_empty() {
        "oxphp-profile".to_string()
    } else {
        tree.trace_id.to_string()
    };

    let doc = json!({
        "$schema": "https://www.speedscope.app/file-format-schema.json",
        "shared": { "frames": frame_array },
        "profiles": [{
            "type": "evented",
            "name": profile_name,
            "unit": "microseconds",
            "startValue": 0,
            "endValue": end_us,
            "events": event_array,
        }],
        "name": "OxPHP Profiler",
        "exporter": format!("oxphp/{}", env!("CARGO_PKG_VERSION")),
        "activeProfileIndex": 0,
    });
    serde_json::to_vec(&doc).expect("speedscope JSON serialisation should not fail")
}
