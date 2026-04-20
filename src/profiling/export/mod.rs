//! Profile exporters.
//!
//! Each exporter is a free function `fn(&SpanTree, opts) -> Vec<u8>`
//! with no I/O. Storage orchestrates them by calling each
//! exporter and writing the bytes to disk / pushing over HTTP.

pub mod collapsed;
pub mod pprof;
pub mod speedscope;
pub mod xhprof;

pub use collapsed::{export_collapsed, CollapsedMetric};
pub use pprof::export_pprof;
pub use speedscope::export_speedscope;
pub use xhprof::{export_xhprof, XhguiMeta, XhprofMode};

use crate::profiling::{FinishedSpan, SpanTree};
use std::collections::HashMap;

/// Build a `span_id → &FinishedSpan` lookup over the tree's
/// finished list. Shared helper for path-walking exporters.
pub(crate) fn index_by_span_id(tree: &SpanTree) -> HashMap<&str, &FinishedSpan> {
    let mut idx = HashMap::with_capacity(tree.finished.len());
    for span in &tree.finished {
        idx.insert(span.span_id.as_ref(), span);
    }
    idx
}

/// Walk `span` up its parent chain, collecting names from leaf to
/// root. Stops when `parent_span_id` equals `tree.root_span_id` or
/// when no entry is found in `idx`.
///
/// Returned vec is leaf-first (caller may reverse if root-first is
/// needed). Always non-empty (the input span itself is included).
pub(crate) fn walk_parent_chain<'a>(
    tree: &'a SpanTree,
    idx: &HashMap<&'a str, &'a FinishedSpan>,
    span: &'a FinishedSpan,
) -> Vec<&'a str> {
    let mut chain = Vec::with_capacity(8);
    let mut cur = span;
    chain.push(cur.name.as_ref());
    while cur.parent_span_id != tree.root_span_id {
        match idx.get(cur.parent_span_id.as_ref()) {
            Some(parent) => {
                chain.push(parent.name.as_ref());
                cur = parent;
            }
            None => break, // orphan — stop walking
        }
    }
    chain
}
