//! Collapsed-stack exporter (Brendan Gregg `flamegraph.pl` format).
//!
//! Output: one line per `FinishedSpan` in the form
//! `root;parent;...;leaf VALUE\n`. The path is leaf-last so
//! `flamegraph.pl` reads it as a stack. Value is the chosen metric.

use crate::profiling::export::{index_by_span_id, walk_parent_chain};
use crate::profiling::SpanTree;

/// Which metric to render as the per-line value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollapsedMetric {
    /// Wall-clock time in microseconds (`(end_ns - start_ns) / 1000`).
    /// Includes every span — wall time is always meaningful.
    Wall,
    /// CPU time in microseconds (`cpu_ns / 1000`). Spans with
    /// `cpu_ns == 0` are skipped (APM-pushed spans without observer
    /// timing capture).
    Cpu,
    /// Memory delta in bytes (`max(0, mem_exit - mem_enter)`).
    /// Spans with non-positive delta are skipped (frees, no-ops).
    Mem,
}

/// Render the tree as collapsed-stack lines for the chosen metric.
/// Returns UTF-8 bytes ready to write to disk / network.
///
/// An empty tree → empty output (no lines, no trailing newline).
pub fn export_collapsed(tree: &SpanTree, metric: CollapsedMetric) -> Vec<u8> {
    let idx = index_by_span_id(tree);
    let mut out: Vec<u8> = Vec::with_capacity(tree.finished.len() * 64);

    for span in &tree.finished {
        let value: u64 = match metric {
            CollapsedMetric::Wall => span.end_ns.saturating_sub(span.start_ns) / 1000,
            CollapsedMetric::Cpu => {
                if span.cpu_ns == 0 {
                    continue;
                }
                span.cpu_ns / 1000
            }
            CollapsedMetric::Mem => {
                let delta = (span.mem_exit - span.mem_enter).max(0);
                if delta <= 0 {
                    continue;
                }
                delta as u64
            }
        };

        let mut chain = walk_parent_chain(tree, &idx, span);
        chain.reverse(); // root-first

        // Synthetic "main()" root — xhprof / xhgui convention; gives
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
