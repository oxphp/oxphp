//! Collapsed-stack exporter (Brendan Gregg `flamegraph.pl` format).
//!
//! Output: one line per distinct call stack in the form
//! `root;parent;...;leaf VALUE\n`, in the order the stacks first finished.
//! The path is leaf-last so `flamegraph.pl` reads it as a stack. Value is
//! the wall-clock time of the calls on that stack without their direct
//! callees, in microseconds: a flamegraph draws a frame as wide as its own
//! line plus every line under it, so a span's whole duration here would be
//! counted again at each level above it.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::profiling::export::{index_by_span_id, walk_parent_chain};
use crate::profiling::{FinishedSpan, SpanTree};

fn wall_ns(span: &FinishedSpan) -> u64 {
    span.end_ns.saturating_sub(span.start_ns)
}

/// Render the tree as collapsed-stack lines.
/// Returns UTF-8 bytes ready to write to disk / network.
///
/// An empty tree → empty output (no lines, no trailing newline).
pub fn export_collapsed(tree: &SpanTree) -> Vec<u8> {
    let idx = index_by_span_id(tree);

    // Saturating: a force-closed span reads as decades long, its end taken
    // from another clock, and a dozen of them under one caller overflow u64.
    let mut callees_ns: HashMap<&str, u64> = HashMap::with_capacity(tree.finished.len());
    for span in &tree.finished {
        let sum = callees_ns.entry(span.parent_span_id.as_ref()).or_insert(0);
        *sum = sum.saturating_add(wall_ns(span));
    }

    // Summed per stack and cut to microseconds once per line: cut per call, a
    // function running under a microsecond is written as 0 however often it
    // runs, and its caller has already given that time up.
    let mut own_ns_by_stack: IndexMap<Vec<u8>, u64> = IndexMap::new();
    for span in &tree.finished {
        let mut chain = walk_parent_chain(tree, &idx, span);
        chain.reverse(); // root-first

        // Synthetic "main()" root — the xhprof convention; gives
        // flamegraph.pl a single-rooted tree even when SpanTree's
        // root_span_id corresponds to no observed span.
        let mut stack = b"main()".to_vec();
        for name in &chain {
            stack.push(b';');
            // Escape `;` and ` ` in names so flamegraph.pl parses
            // unambiguously. PHP function names can't contain space
            // and rarely contain `;`; the escape is defensive.
            for byte in name.bytes() {
                match byte {
                    b';' => stack.extend_from_slice(b"\\;"),
                    b' ' => stack.extend_from_slice(b"\\ "),
                    _ => stack.push(byte),
                }
            }
        }

        let callees = callees_ns.get(span.span_id.as_ref()).copied().unwrap_or(0);
        let own_ns = wall_ns(span).saturating_sub(callees);
        let total = own_ns_by_stack.entry(stack).or_insert(0);
        *total = total.saturating_add(own_ns);
    }

    let mut out: Vec<u8> = Vec::with_capacity(own_ns_by_stack.len() * 64);
    for (stack, own_ns) in &own_ns_by_stack {
        out.extend_from_slice(stack);
        out.extend_from_slice(format!(" {}\n", own_ns / 1000).as_bytes());
    }
    out
}
