//! Cycle detection for nested Shareable references inside container types.
//!
//! BFS from `start` with a visited set and two bounds (`max_depth`,
//! `max_edges`). Called on every container write whose value is
//! `SharedValue::Shared` (used by `Shared\Map::set`).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::plugins::ox_shared::registry::SharedId;
use crate::plugins::ox_shared::value::SharedRef;

/// Outcome of [`would_create_cycle`] when the insertion is not safe.
#[derive(Debug, PartialEq, Eq)]
pub enum CycleError {
    /// A path `start → ... → target` exists. The path is ordered from
    /// `start` to `target` inclusive. Used to build the human-readable
    /// `CycleException` message.
    CycleFound(Vec<SharedId>),
    /// BFS hit `max_depth` without either finding the target or
    /// exhausting the graph. Operator can raise
    /// `SHARED_CYCLE_DETECT_DEPTH` if the graph is legitimately deep.
    DepthExceeded,
    /// Total edges walked exceeded `max_edges` (guard against dense
    /// graphs). Operator can raise `SHARED_CYCLE_DETECT_EDGES`.
    EdgeLimitExceeded,
}

/// Decide whether adding an edge `target → start` would introduce a
/// cycle in the reachability graph.
///
/// The walker explores forward edges from `start` looking for `target`;
/// if reachable, the edge `target → start` would close a cycle. This is
/// the contract point called from `Shared\Map::set($key, $sharedable)`
/// with `start = ref(sharedable)` and `target = map_id`.
///
/// `children_of(parent_id, &mut out)` enumerates outgoing [`SharedRef`]
/// edges of `parent_id` by pushing them into `out`. The walker clears
/// `out` before each call, so producers do not need to.
///
/// Complexity is `O(V + E)` bounded by `max_depth` and `max_edges` —
/// the visited set guarantees each node is enqueued at most once, so
/// there is no exponential blow-up even on dense fan-out graphs.
pub fn would_create_cycle<F>(
    start: SharedRef,
    target: SharedId,
    max_depth: usize,
    max_edges: usize,
    mut children_of: F,
) -> Result<(), CycleError>
where
    F: FnMut(SharedId, &mut Vec<SharedRef>),
{
    let mut queue: VecDeque<(SharedId, usize)> = VecDeque::new();
    let mut visited: HashSet<SharedId> = HashSet::new();
    let mut parent: HashMap<SharedId, SharedId> = HashMap::new();
    let mut buf: Vec<SharedRef> = Vec::new();
    let mut edges_walked: usize = 0;

    queue.push_back((start.id, 0));
    visited.insert(start.id);

    while let Some((id, depth)) = queue.pop_front() {
        if id == target {
            return Err(CycleError::CycleFound(reconstruct_path(
                &parent, target, start.id,
            )));
        }
        if depth >= max_depth {
            return Err(CycleError::DepthExceeded);
        }

        buf.clear();
        children_of(id, &mut buf);
        for child in buf.drain(..) {
            edges_walked += 1;
            if edges_walked > max_edges {
                return Err(CycleError::EdgeLimitExceeded);
            }
            if visited.insert(child.id) {
                parent.insert(child.id, id);
                queue.push_back((child.id, depth + 1));
            }
        }
    }

    Ok(())
}

fn reconstruct_path(
    parent: &HashMap<SharedId, SharedId>,
    target: SharedId,
    start: SharedId,
) -> Vec<SharedId> {
    let mut path = vec![target];
    if target == start {
        return path;
    }
    let mut cur = target;
    while let Some(&p) = parent.get(&cur) {
        path.push(p);
        if p == start {
            break;
        }
        cur = p;
    }
    path.reverse();
    path
}

/// Format a cycle path for inclusion in a `CycleException` message.
/// Produces `"#1 → #4 → #1"` style output.
pub fn format_cycle_path(path: &[SharedId]) -> String {
    let mut out = String::with_capacity(path.len() * 6);
    for (i, id) in path.iter().enumerate() {
        if i > 0 {
            out.push_str(" → ");
        }
        out.push('#');
        out.push_str(&id.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::registry::SharedType;

    fn sref(id: SharedId) -> SharedRef {
        SharedRef {
            id,
            type_tag: SharedType::Map,
        }
    }

    /// Build a `children_of` closure from a static edge list.
    fn graph_fn(
        edges: Vec<(SharedId, Vec<SharedId>)>,
    ) -> impl FnMut(SharedId, &mut Vec<SharedRef>) {
        let map: HashMap<SharedId, Vec<SharedRef>> = edges
            .into_iter()
            .map(|(k, vs)| (k, vs.into_iter().map(sref).collect()))
            .collect();
        move |id, out| {
            if let Some(refs) = map.get(&id) {
                out.extend_from_slice(refs);
            }
        }
    }

    #[test]
    fn no_cycle_single_node() {
        let r = would_create_cycle(sref(1), 2, 16, 10_000, graph_fn(vec![]));
        assert_eq!(r, Ok(()));
    }

    #[test]
    fn direct_self_reference() {
        // Inserting the map into itself: start.id == target.
        let r = would_create_cycle(sref(1), 1, 16, 10_000, graph_fn(vec![]));
        assert_eq!(r, Err(CycleError::CycleFound(vec![1])));
    }

    #[test]
    fn two_node_cycle() {
        // Graph: B(2) → A(1). User wants A.set(k, B) — walker called with
        // start=B, target=A → cycle found via edge B → A.
        let r = would_create_cycle(sref(2), 1, 16, 10_000, graph_fn(vec![(2, vec![1])]));
        assert_eq!(r, Err(CycleError::CycleFound(vec![2, 1])));
    }

    #[test]
    fn three_node_cycle_path() {
        // Graph: C(3) → B(2) → A(1). Walker from C targeting A.
        let r = would_create_cycle(
            sref(3),
            1,
            16,
            10_000,
            graph_fn(vec![(3, vec![2]), (2, vec![1])]),
        );
        assert_eq!(r, Err(CycleError::CycleFound(vec![3, 2, 1])));
    }

    #[test]
    fn independent_subgraphs_no_cycle() {
        // Graph: A(1) → X(10) → Y(11). Target B(2) is unreachable.
        let r = would_create_cycle(
            sref(1),
            2,
            16,
            10_000,
            graph_fn(vec![(1, vec![10]), (10, vec![11])]),
        );
        assert_eq!(r, Ok(()));
    }

    #[test]
    fn visited_set_prevents_exponential_blowup() {
        // Diamond-shaped DAG: A→B, A→C, B→D, C→D. D is visited once.
        let mut call_count = 0;
        let mut edges_base = graph_fn(vec![
            (1, vec![2, 3]),
            (2, vec![4]),
            (3, vec![4]),
            (4, vec![]),
        ]);
        let counted = |id, out: &mut Vec<SharedRef>| {
            call_count += 1;
            edges_base(id, out);
        };
        let r = would_create_cycle(sref(1), 999, 16, 10_000, counted);
        assert_eq!(r, Ok(()));
        // Each node enumerated at most once: A, B, C, D = 4 calls.
        assert_eq!(call_count, 4);
    }

    #[test]
    fn depth_exceeded_stops_walk() {
        // Long chain: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8.
        // max_depth=3 means after popping depth-3 node we return DepthExceeded.
        let edges = (1..8u64).map(|i| (i, vec![i + 1])).collect();
        let r = would_create_cycle(sref(1), 999, 3, 10_000, graph_fn(edges));
        assert_eq!(r, Err(CycleError::DepthExceeded));
    }

    #[test]
    fn edge_limit_triggers() {
        // One node with 5 children; max_edges=3 should trip.
        let r = would_create_cycle(
            sref(1),
            999,
            16,
            3,
            graph_fn(vec![(1, vec![10, 11, 12, 13, 14])]),
        );
        assert_eq!(r, Err(CycleError::EdgeLimitExceeded));
    }

    #[test]
    fn cycle_found_before_depth_limit_wins() {
        // Chain 1 → 2 → 3 → 1 (target=1). Even with max_depth small the
        // cycle should be detected first.
        let r = would_create_cycle(
            sref(2),
            1,
            16,
            10_000,
            graph_fn(vec![(2, vec![3]), (3, vec![1])]),
        );
        assert_eq!(r, Err(CycleError::CycleFound(vec![2, 3, 1])));
    }

    #[test]
    fn format_cycle_path_basic() {
        assert_eq!(format_cycle_path(&[1]), "#1");
        assert_eq!(format_cycle_path(&[1, 2, 3]), "#1 → #2 → #3");
        assert_eq!(format_cycle_path(&[]), "");
    }
}
