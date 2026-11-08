//! Collapsed-stack exporter (Brendan Gregg `flamegraph.pl` format).
//!
//! Output: one line per `FinishedSpan` in the form
//! `root;parent;...;leaf VALUE\n`. The path is leaf-last so
//! `flamegraph.pl` reads it as a stack. Value is the span's wall-clock
//! time in microseconds.

use crate::profiling::export::{index_by_span_id, walk_parent_chain};
use crate::profiling::SpanTree;

/// Render the tree as collapsed-stack lines.
/// Returns UTF-8 bytes ready to write to disk / network.
///
/// An empty tree → empty output (no lines, no trailing newline).
pub fn export_collapsed(tree: &SpanTree) -> Vec<u8> {
    let idx = index_by_span_id(tree);
    let mut out: Vec<u8> = Vec::with_capacity(tree.finished.len() * 64);

    for span in &tree.finished {
        let value = span.end_ns.saturating_sub(span.start_ns) / 1000;

        let mut chain = walk_parent_chain(tree, &idx, span);
        chain.reverse(); // root-first

        // Synthetic "main()" root — the xhprof convention; gives
        // flamegraph.pl a single-rooted tree even when SpanTree's
        // root_span_id corresponds to no observed span.
        out.extend_from_slice(b"main()");
        for name in &chain {
            out.push(b';');
            // Escape `;` and ` ` in names so flamegraph.pl parses
            // unambiguously. PHP function names can't contain space
            // and rarely contain `;`; the escape is defensive.
            for byte in name.bytes() {
                match byte {
                    b';' => out.extend_from_slice(b"\\;"),
                    b' ' => out.extend_from_slice(b"\\ "),
                    _ => out.push(byte),
                }
            }
        }
        out.extend_from_slice(format!(" {value}\n").as_bytes());
    }
    out
}
