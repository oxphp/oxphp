//! /__ox_shared/* API endpoints + Prometheus metrics collector.

use bytes::Bytes;
use http::{Response, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::plugin::handler::{PluginInternalRequest, PluginMetricsCollector};
use crate::plugin::{PluginContext, PluginError};
use crate::plugins::ox_shared::registry::{Entry, SharedType, REGISTRY};
use crate::plugins::ox_shared::types::channel::SharedInnerChannelExt;
use crate::plugins::ox_shared::types::map::SharedInnerMapExt;
use crate::plugins::ox_shared::types::pool::SharedInnerPoolExt;
use crate::plugins::ox_shared::value::{SharedRef, SharedValue};
use crate::types::{full_body, ResponseBody};

pub fn register_routes(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.internal_route("/__ox_shared/summary", handle_summary);
    ctx.internal_route("/__ox_shared/entries", handle_entries);
    ctx.internal_route("/__ox_shared/entry", handle_entry_by_id);
    ctx.internal_route("/__ox_shared/types", handle_types);
    ctx.internal_route("/__ox_shared/preview", handle_preview);
    ctx.internal_route("/__ox_shared/graph", handle_graph);
    Ok(())
}

fn handle_summary(_req: &PluginInternalRequest) -> Response<ResponseBody> {
    let Some(reg) = REGISTRY.get() else {
        return json_response(500, json!({"error": "registry not initialised"}));
    };

    let cfg = reg.config();
    let entries = reg.total_entries();
    let bytes = reg.total_bytes();

    let mut by_type: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
    for e in reg.iter_entries() {
        let name = e.type_tag.name();
        let slot = by_type.entry(name).or_default();
        slot.0 += 1;
        slot.1 += e.mem_bytes.load(Ordering::Relaxed) as u64;
        slot.2 += e.ops.load(Ordering::Relaxed);
    }
    let by_type_json: Value = by_type
        .into_iter()
        .map(|(k, (count, bytes, ops))| {
            (
                k.to_string(),
                json!({"count": count, "bytes": bytes, "ops": ops}),
            )
        })
        .collect::<serde_json::Map<String, Value>>()
        .into();

    let saturation_entries = if cfg.max_entries == 0 {
        0.0
    } else {
        entries as f64 / cfg.max_entries as f64
    };
    let saturation_bytes = if cfg.max_bytes == 0 {
        0.0
    } else {
        bytes as f64 / cfg.max_bytes as f64
    };

    let body = json!({
        "total_entries": entries,
        "total_bytes": bytes,
        "by_type": by_type_json,
        "limits": {
            "max_entries": cfg.max_entries,
            "max_bytes": cfg.max_bytes,
            "soft_ratio": cfg.soft_limit_ratio,
        },
        "saturation": {
            "entries": saturation_entries,
            "bytes": saturation_bytes,
        },
        "diagnostics": {
            "lock_diagnostics_level": format!("{:?}", cfg.lock_diagnostics).to_lowercase(),
            "cycle_detect_depth": cfg.cycle_detect_depth,
            "poison_strict": cfg.poison_strict,
        }
    });
    json_response(200, body)
}

fn handle_entries(req: &PluginInternalRequest) -> Response<ResponseBody> {
    let Some(reg) = REGISTRY.get() else {
        return json_response(500, json!({"error": "registry not initialised"}));
    };

    let limit: usize = req
        .query
        .and_then(|q| extract_query_param(q, "limit"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .min(500);

    let items: Vec<Value> = reg
        .iter_entries()
        .take(limit)
        .map(|e| {
            json!({
                "id": e.id,
                "type": e.type_tag.name(),
                "refcount": Arc::strong_count(&e) as u64,
                "ops": e.ops.load(Ordering::Relaxed),
                "mem_bytes": e.mem_bytes.load(Ordering::Relaxed),
                "age_sec": e.created_at.elapsed().as_secs(),
            })
        })
        .collect();

    json_response(
        200,
        json!({
            "items": items,
            "next_cursor": Value::Null,
            "total_matching": reg.total_entries(),
        }),
    )
}

fn handle_entry_by_id(req: &PluginInternalRequest) -> Response<ResponseBody> {
    let Some(id) = req
        .query
        .and_then(|q| extract_query_param(q, "id"))
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return json_response(400, json!({"error": "missing ?id="}));
    };
    let Some(reg) = REGISTRY.get() else {
        return json_response(500, json!({"error": "registry not initialised"}));
    };
    match reg.lookup(id) {
        Ok(e) => json_response(200, entry_to_json(&e)),
        Err(_) => json_response(404, json!({"error": "not found"})),
    }
}

fn handle_preview(req: &PluginInternalRequest) -> Response<ResponseBody> {
    let Some(id) = req
        .query
        .and_then(|q| extract_query_param(q, "id"))
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return json_response(400, json!({"error": "missing ?id="}));
    };
    let Some(reg) = REGISTRY.get() else {
        return json_response(500, json!({"error": "registry not initialised"}));
    };
    let cfg = reg.config();
    if !cfg.introspection_preview_enabled {
        return json_response(403, json!({"error": "preview disabled"}));
    }

    match reg.lookup(id) {
        Ok(e) => {
            let snap = e.inner.debug_snapshot();
            let preview = render_preview(&snap, cfg.preview_string_limit, cfg.preview_array_limit);
            json_response(
                200,
                json!({
                    "id": e.id,
                    "type": e.type_tag.name(),
                    "preview": preview,
                }),
            )
        }
        Err(_) => json_response(404, json!({"error": "not found"})),
    }
}

fn render_preview(sv: &SharedValue, str_limit: usize, arr_limit: usize) -> String {
    match sv {
        SharedValue::Null => "null".to_string(),
        SharedValue::Bool(true) => "true".to_string(),
        SharedValue::Bool(false) => "false".to_string(),
        SharedValue::Long(v) => v.to_string(),
        SharedValue::Double(v) => v.to_string(),
        SharedValue::String(s) => {
            if s.len() > str_limit {
                format!("\"{}…\" ({} bytes)", &s[..str_limit], s.len())
            } else {
                format!("\"{s}\"")
            }
        }
        SharedValue::Bytes(b) => format!("<{} bytes>", b.len()),
        SharedValue::Array(arr) => {
            let total = arr.int_keyed.len() + arr.str_keyed.len();
            let shown = total.min(arr_limit);
            format!("[... {shown}/{total} entries elided]")
        }
        SharedValue::Shared(r) => format!("&Shared\\{}({})", r.type_tag.name(), r.id),
    }
}

fn entry_to_json(e: &Arc<Entry>) -> Value {
    let type_specific = match e.type_tag {
        SharedType::Counter => match e.inner.debug_snapshot() {
            SharedValue::Long(v) => json!({"value": v}),
            _ => Value::Null,
        },
        SharedType::Atomic => match e.inner.debug_snapshot() {
            SharedValue::Long(v) => json!({"value": v}),
            _ => Value::Null,
        },
        SharedType::Flag => match e.inner.debug_snapshot() {
            SharedValue::Bool(b) => json!({"set": b}),
            _ => Value::Null,
        },
        SharedType::Once => {
            let init = !matches!(e.inner.debug_snapshot(), SharedValue::Null);
            json!({"initialized": init})
        }
        SharedType::Channel => {
            if let Some(ch) = e.inner.as_any_channel() {
                json!({
                    "capacity": ch.capacity(),
                    "pending": ch.pending(),
                    "closed": ch.is_closed(),
                    "senders_blocked": ch.senders_blocked().load(Ordering::Relaxed),
                    "receivers_blocked": ch.receivers_blocked().load(Ordering::Relaxed),
                })
            } else {
                Value::Null
            }
        }
        SharedType::Map => {
            if let Some(map) = e.inner.as_any_map() {
                let count = map.count();
                let max = map.max_entries();
                let saturation = match max {
                    Some(m) if m > 0 => count as f64 / m as f64,
                    _ => 0.0,
                };
                let sample_limit = REGISTRY
                    .get()
                    .map(|r| r.config().preview_array_limit)
                    .unwrap_or(20);
                let sample: Vec<String> = map
                    .keys()
                    .iter()
                    .take(sample_limit)
                    .map(|s| s.to_string())
                    .collect();
                json!({
                    "key_count": count,
                    "max_entries": max.map(|m| m as u64),
                    "saturation": saturation,
                    "sample_keys": sample,
                })
            } else {
                Value::Null
            }
        }
        SharedType::Pool => {
            if let Some(pool) = e.inner.as_any_pool() {
                let size = pool.size();
                let idle = pool.idle_count() as u64;
                let in_use = size.saturating_sub(idle);
                // Per-thread idle snapshot keyed by the raw pthread
                // id as a stringified u64 — matches the spec's
                // `{"t0": N, "t1": M}` shape (thread labels are
                // opaque to the consumer; only the distribution is
                // meaningful).
                let mut idle_by_thread = serde_json::Map::new();
                for (k, n) in pool.idle_by_thread() {
                    idle_by_thread.insert(k.to_string(), json!(n as u64));
                }
                json!({
                    "max_size": pool.max_size() as u64,
                    "size": size,
                    "in_use": in_use,
                    "idle": idle,
                    "waiting": pool.waiting_count(),
                    "idle_by_thread": idle_by_thread,
                    "rebalance_strategy": "strict",
                })
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    };
    json!({
        "id": e.id,
        "type": e.type_tag.name(),
        "refcount": Arc::strong_count(e) as u64,
        "ops": e.ops.load(Ordering::Relaxed),
        "mem_bytes": e.mem_bytes.load(Ordering::Relaxed),
        "age_sec": e.created_at.elapsed().as_secs(),
        "type_specific": type_specific,
    })
}

fn handle_types(_req: &PluginInternalRequest) -> Response<ResponseBody> {
    let types = vec![
        json!({"tag": 10, "name": "Counter", "php_class": "OxPHP\\Shared\\Counter"}),
        json!({"tag": 11, "name": "Flag",    "php_class": "OxPHP\\Shared\\Flag"}),
        json!({"tag": 12, "name": "Once",    "php_class": "OxPHP\\Shared\\Once"}),
        json!({"tag": 13, "name": "Atomic",  "php_class": "OxPHP\\Shared\\Atomic"}),
        json!({"tag": 20, "name": "Map",     "php_class": "OxPHP\\Shared\\Map"}),
        json!({"tag": 31, "name": "Channel", "php_class": "OxPHP\\Shared\\Channel"}),
        json!({"tag": 50, "name": "Pool",    "php_class": "OxPHP\\Shared\\Pool"}),
    ];
    json_response(200, json!({"types": types}))
}

/// `/__ox_shared/graph?id=N[&depth=D][&edges=E]` — BFS walk of the
/// reachability graph rooted at `N`. Returns nodes + edges for the
/// reachable subgraph. Useful for debugging cycle-detection rejections
/// or spotting orphaned nested `Shareable` chains.
///
/// Defaults: `depth=16`, `edges=500`. Truncated flag signals the walker
/// hit a bound (node set may be incomplete).
fn handle_graph(req: &PluginInternalRequest) -> Response<ResponseBody> {
    let Some(id) = req
        .query
        .and_then(|q| extract_query_param(q, "id"))
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return json_response(400, json!({"error": "missing ?id="}));
    };
    let Some(reg) = REGISTRY.get() else {
        return json_response(500, json!({"error": "registry not initialised"}));
    };

    let depth_limit: usize = req
        .query
        .and_then(|q| extract_query_param(q, "depth"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let edge_limit: usize = req
        .query
        .and_then(|q| extract_query_param(q, "edges"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);

    if reg.lookup(id).is_err() {
        return json_response(404, json!({"error": "not found"}));
    }

    let mut visited: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<(u64, usize)> = std::collections::VecDeque::new();
    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    let mut buf: Vec<SharedRef> = Vec::new();
    let mut truncated = false;

    visited.insert(id);
    queue.push_back((id, 0));

    while let Some((cur, d)) = queue.pop_front() {
        let Ok(entry) = reg.lookup(cur) else { continue };
        nodes.push(json!({
            "id": cur,
            "type": entry.type_tag.name(),
            "refcount": Arc::strong_count(&entry) as u64,
            "mem_bytes": entry.mem_bytes.load(Ordering::Relaxed),
        }));
        if d >= depth_limit {
            truncated = true;
            continue;
        }
        buf.clear();
        entry.inner.children(&mut buf);
        for child in buf.drain(..) {
            if edges.len() >= edge_limit {
                truncated = true;
                break;
            }
            edges.push(json!({"from": cur, "to": child.id}));
            if visited.insert(child.id) {
                queue.push_back((child.id, d + 1));
            }
        }
        if edges.len() >= edge_limit {
            truncated = true;
            break;
        }
    }

    json_response(
        200,
        json!({
            "root": id,
            "nodes": nodes,
            "edges": edges,
            "truncated": truncated,
            "limits": {"depth": depth_limit, "edges": edge_limit},
        }),
    )
}

fn extract_query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v);
            }
        }
    }
    None
}

fn json_response(status: u16, body: Value) -> Response<ResponseBody> {
    let bytes = body.to_string().into_bytes();
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header("content-type", "application/json")
        .body(full_body(Bytes::from(bytes)))
        .unwrap()
}

// ─── Prometheus metrics collector ───────────────────────────────────────────

pub struct SharedMetricsCollector;

impl PluginMetricsCollector for SharedMetricsCollector {
    fn collect(&self, output: &mut String) {
        let Some(reg) = REGISTRY.get() else {
            return;
        };
        let cfg = reg.config();
        let entries = reg.total_entries();
        let bytes = reg.total_bytes();

        let mut by_type: BTreeMap<&str, u64> = BTreeMap::new();
        let mut ops_by_type: BTreeMap<&str, u64> = BTreeMap::new();
        let mut bytes_by_type: BTreeMap<&str, u64> = BTreeMap::new();
        for e in reg.iter_entries() {
            let n = e.type_tag.name();
            *by_type.entry(n).or_default() += 1;
            *ops_by_type.entry(n).or_default() += e.ops.load(Ordering::Relaxed);
            *bytes_by_type.entry(n).or_default() += e.mem_bytes.load(Ordering::Relaxed) as u64;
        }

        output.push_str(
            "# HELP oxphp_shared_objects_total Count of active Shared\\* entries by type.\n",
        );
        output.push_str("# TYPE oxphp_shared_objects_total gauge\n");
        for (t, n) in &by_type {
            output.push_str(&format!("oxphp_shared_objects_total{{type=\"{t}\"}} {n}\n"));
        }

        output.push_str("# HELP oxphp_shared_operations_total Total operations per type.\n");
        output.push_str("# TYPE oxphp_shared_operations_total counter\n");
        for (t, n) in &ops_by_type {
            output.push_str(&format!(
                "oxphp_shared_operations_total{{type=\"{t}\"}} {n}\n"
            ));
        }

        output.push_str("# HELP oxphp_shared_bytes Approximate byte usage per type.\n");
        output.push_str("# TYPE oxphp_shared_bytes gauge\n");
        for (t, n) in &bytes_by_type {
            output.push_str(&format!("oxphp_shared_bytes{{type=\"{t}\"}} {n}\n"));
        }

        output.push_str("# HELP oxphp_shared_total_bytes Total approximate Shared memory.\n");
        output.push_str("# TYPE oxphp_shared_total_bytes gauge\n");
        output.push_str(&format!("oxphp_shared_total_bytes {bytes}\n"));

        output.push_str("# HELP oxphp_shared_capacity_saturation Fraction of capacity used.\n");
        output.push_str("# TYPE oxphp_shared_capacity_saturation gauge\n");
        let sat_entries = if cfg.max_entries == 0 {
            0.0f64
        } else {
            entries as f64 / cfg.max_entries as f64
        };
        let sat_bytes = if cfg.max_bytes == 0 {
            0.0f64
        } else {
            bytes as f64 / cfg.max_bytes as f64
        };
        output.push_str(&format!(
            "oxphp_shared_capacity_saturation{{kind=\"entries\"}} {sat_entries:.4}\n"
        ));
        output.push_str(&format!(
            "oxphp_shared_capacity_saturation{{kind=\"bytes\"}} {sat_bytes:.4}\n"
        ));

        // Deadlock detector: cumulative cycles observed.
        output.push_str(
            "# HELP oxphp_shared_deadlock_detected_total Cross-thread cycles detected by the wait-for scanner.\n",
        );
        output.push_str("# TYPE oxphp_shared_deadlock_detected_total counter\n");
        output.push_str(&format!(
            "oxphp_shared_deadlock_detected_total {}\n",
            crate::plugins::ox_shared::deadlock::cycles_detected_total()
        ));

        // Per-channel metrics.
        output.push_str(
            "# HELP oxphp_shared_channel_pending Current items buffered in each Channel.\n",
        );
        output.push_str("# TYPE oxphp_shared_channel_pending gauge\n");

        output.push_str(
            "# HELP oxphp_shared_channel_senders_blocked Senders currently blocked or fiber-suspended on each Channel.\n",
        );
        output.push_str("# TYPE oxphp_shared_channel_senders_blocked gauge\n");

        output.push_str(
            "# HELP oxphp_shared_channel_receivers_blocked Receivers currently blocked or fiber-suspended on each Channel.\n",
        );
        output.push_str("# TYPE oxphp_shared_channel_receivers_blocked gauge\n");

        output.push_str(
            "# HELP oxphp_shared_channel_items_sent_total Cumulative successful sends per Channel.\n",
        );
        output.push_str("# TYPE oxphp_shared_channel_items_sent_total counter\n");

        output.push_str(
            "# HELP oxphp_shared_channel_items_dropped_total Cumulative items dropped due to cancelled waiter races.\n",
        );
        output.push_str("# TYPE oxphp_shared_channel_items_dropped_total counter\n");

        for e in reg
            .iter_entries()
            .filter(|e| e.type_tag == SharedType::Channel)
        {
            if let Some(ch) = e.inner.as_any_channel() {
                let id = e.id;
                output.push_str(&format!(
                    "oxphp_shared_channel_pending{{channel_id=\"{id}\"}} {}\n",
                    ch.pending()
                ));
                output.push_str(&format!(
                    "oxphp_shared_channel_senders_blocked{{channel_id=\"{id}\"}} {}\n",
                    ch.senders_blocked().load(Ordering::Relaxed)
                ));
                output.push_str(&format!(
                    "oxphp_shared_channel_receivers_blocked{{channel_id=\"{id}\"}} {}\n",
                    ch.receivers_blocked().load(Ordering::Relaxed)
                ));
                output.push_str(&format!(
                    "oxphp_shared_channel_items_sent_total{{channel_id=\"{id}\"}} {}\n",
                    ch.items_sent_total().load(Ordering::Relaxed)
                ));
                output.push_str(&format!(
                    "oxphp_shared_channel_items_dropped_total{{channel_id=\"{id}\"}} {}\n",
                    ch.items_dropped_total().load(Ordering::Relaxed)
                ));
            }
        }

        // Per-map metrics.
        output.push_str(
            "# HELP oxphp_shared_map_entries Current key count per Shared\\Map instance.\n",
        );
        output.push_str("# TYPE oxphp_shared_map_entries gauge\n");
        output.push_str(
            "# HELP oxphp_shared_map_max_entries Configured per-instance cap (0 = unbounded).\n",
        );
        output.push_str("# TYPE oxphp_shared_map_max_entries gauge\n");
        output.push_str(
            "# HELP oxphp_shared_map_saturation Fraction of per-instance cap used (0 when unbounded).\n",
        );
        output.push_str("# TYPE oxphp_shared_map_saturation gauge\n");

        for e in reg.iter_entries().filter(|e| e.type_tag == SharedType::Map) {
            if let Some(map) = e.inner.as_any_map() {
                let id = e.id;
                let count = map.count() as u64;
                let max = map.max_entries().unwrap_or(0) as u64;
                let sat = if max == 0 {
                    0.0
                } else {
                    count as f64 / max as f64
                };
                output.push_str(&format!(
                    "oxphp_shared_map_entries{{map_id=\"{id}\"}} {count}\n"
                ));
                output.push_str(&format!(
                    "oxphp_shared_map_max_entries{{map_id=\"{id}\"}} {max}\n"
                ));
                output.push_str(&format!(
                    "oxphp_shared_map_saturation{{map_id=\"{id}\"}} {sat:.4}\n"
                ));
            }
        }

        // Per-pool metrics. Gauges first, then the outcome + wait
        // histogram + eviction counters.
        output.push_str(
            "# HELP oxphp_shared_pool_size Authoritative capacity gauge (in_use + idle).\n",
        );
        output.push_str("# TYPE oxphp_shared_pool_size gauge\n");
        output
            .push_str("# HELP oxphp_shared_pool_in_use Slots currently checked out by callers.\n");
        output.push_str("# TYPE oxphp_shared_pool_in_use gauge\n");
        output.push_str("# HELP oxphp_shared_pool_idle Slots parked in owner idle deques.\n");
        output.push_str("# TYPE oxphp_shared_pool_idle gauge\n");
        output.push_str("# HELP oxphp_shared_pool_waiting Callers blocked on wait_for_release.\n");
        output.push_str("# TYPE oxphp_shared_pool_waiting gauge\n");
        output.push_str("# HELP oxphp_shared_pool_acquire_total Cumulative acquire results.\n");
        output.push_str("# TYPE oxphp_shared_pool_acquire_total counter\n");
        output.push_str(
            "# HELP oxphp_shared_pool_wait_seconds Acquire-call wait distribution, in seconds.\n",
        );
        output.push_str("# TYPE oxphp_shared_pool_wait_seconds histogram\n");
        output.push_str(
            "# HELP oxphp_shared_pool_evicted_total Cumulative destroy invocations, by reason.\n",
        );
        output.push_str("# TYPE oxphp_shared_pool_evicted_total counter\n");

        // Bucket ceilings must match `PoolInner::wait_histogram_snapshot`.
        const WAIT_BUCKET_LE: [&str; 6] = ["0.001", "0.01", "0.1", "1", "10", "+Inf"];

        for e in reg
            .iter_entries()
            .filter(|e| e.type_tag == SharedType::Pool)
        {
            if let Some(pool) = e.inner.as_any_pool() {
                let id = e.id;
                let size = pool.size();
                let idle = pool.idle_count() as u64;
                let in_use = size.saturating_sub(idle);
                let waiting = pool.waiting_count();
                output.push_str(&format!(
                    "oxphp_shared_pool_size{{pool_id=\"{id}\"}} {size}\n"
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_in_use{{pool_id=\"{id}\"}} {in_use}\n"
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_idle{{pool_id=\"{id}\"}} {idle}\n"
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_waiting{{pool_id=\"{id}\"}} {waiting}\n"
                ));

                // acquire_total{result=...}
                output.push_str(&format!(
                    "oxphp_shared_pool_acquire_total{{pool_id=\"{id}\",result=\"ok\"}} {}\n",
                    pool.acquire_ok_total()
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_acquire_total{{pool_id=\"{id}\",result=\"timeout\"}} {}\n",
                    pool.acquire_timeout_total()
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_acquire_total{{pool_id=\"{id}\",result=\"closed\"}} {}\n",
                    pool.acquire_closed_total()
                ));

                // wait_seconds histogram (cumulative buckets + sum + count).
                let (cum, sum_s, count) = pool.wait_histogram_snapshot();
                for (i, le) in WAIT_BUCKET_LE.iter().enumerate() {
                    output.push_str(&format!(
                        "oxphp_shared_pool_wait_seconds_bucket{{pool_id=\"{id}\",le=\"{le}\"}} {}\n",
                        cum[i]
                    ));
                }
                output.push_str(&format!(
                    "oxphp_shared_pool_wait_seconds_sum{{pool_id=\"{id}\"}} {sum_s:.6}\n"
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_wait_seconds_count{{pool_id=\"{id}\"}} {count}\n"
                ));

                // evicted_total{reason=...}
                output.push_str(&format!(
                    "oxphp_shared_pool_evicted_total{{pool_id=\"{id}\",reason=\"idle_timeout\"}} {}\n",
                    pool.evicted_idle_total()
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_evicted_total{{pool_id=\"{id}\",reason=\"evict\"}} {}\n",
                    pool.evicted_manual_total()
                ));
                output.push_str(&format!(
                    "oxphp_shared_pool_evicted_total{{pool_id=\"{id}\",reason=\"shutdown\"}} {}\n",
                    pool.evicted_shutdown_total()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
    use crate::plugins::ox_shared::registry::{init_registry, registry};
    use crate::plugins::ox_shared::types::channel::ChannelInner;
    use http::{HeaderMap, Method};
    use http_body_util::BodyExt;

    fn ensure_test_registry() {
        init_registry(SharedConfig {
            enabled: true,
            max_entries: 10_000,
            max_bytes: 1 << 30,
            soft_limit_ratio: 0.7,
            metrics_enabled: true,
            introspection_enabled: true,
            introspection_preview_enabled: true,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
    }

    #[test]
    fn channel_entry_json_has_type_specific() {
        ensure_test_registry();
        let reg = registry();
        let entry = reg
            .insert(SharedType::Channel, Arc::new(ChannelInner::new(16)))
            .expect("insert channel");
        let id = entry.id;

        let v = entry_to_json(&entry);
        assert_eq!(v["type"], "Channel");
        assert_eq!(v["id"], id);

        let ts = &v["type_specific"];
        assert_eq!(ts["capacity"], 16);
        assert_eq!(ts["pending"], 0);
        assert_eq!(ts["closed"], false);
        assert_eq!(ts["senders_blocked"], 0);
        assert_eq!(ts["receivers_blocked"], 0);

        // Drop the strong ref so later tests don't see a stale entry —
        // Entry::Drop self-deregisters when the last Arc dies.
        drop(entry);
    }

    #[tokio::test]
    async fn types_endpoint_includes_channel() {
        ensure_test_registry();
        let method = Method::GET;
        let headers = HeaderMap::new();
        let req = PluginInternalRequest {
            method: &method,
            path: "/__ox_shared/types",
            headers: &headers,
            query: None,
        };
        let resp = handle_types(&req);
        assert_eq!(resp.status().as_u16(), 200);
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        let types = v["types"].as_array().expect("types array");
        let ch = types
            .iter()
            .find(|e| e["name"] == "Channel")
            .expect("Channel entry");
        assert_eq!(ch["tag"], 31);
        assert_eq!(ch["php_class"], "OxPHP\\Shared\\Channel");
    }

    #[test]
    fn map_entry_json_has_type_specific() {
        use crate::plugins::ox_shared::types::map::MapInner;
        use crate::plugins::ox_shared::value::SharedValue as SV;

        ensure_test_registry();
        let reg = registry();
        let inner: Arc<dyn crate::plugins::ox_shared::registry::SharedInner> =
            Arc::new(MapInner::new(Some(100)));
        let entry = reg.insert(SharedType::Map, Arc::clone(&inner)).unwrap();
        let id = entry.id;
        let concrete = (*inner).as_any_map().unwrap();
        concrete.bind_id(id);
        for i in 0..3 {
            concrete
                .set(Arc::from(format!("k{i}")), SV::Long(i))
                .unwrap();
        }

        let v = entry_to_json(&entry);
        assert_eq!(v["type"], "Map");
        let ts = &v["type_specific"];
        assert_eq!(ts["key_count"], 3);
        assert_eq!(ts["max_entries"], 100);
        let sat = ts["saturation"].as_f64().unwrap();
        assert!((sat - 0.03).abs() < 1e-9);
        let samples = ts["sample_keys"].as_array().unwrap();
        assert_eq!(samples.len(), 3);

        drop(inner);
        drop(entry);
    }

    #[test]
    fn pool_entry_json_has_type_specific() {
        use crate::plugins::ox_shared::types::pool::PoolInner;
        use std::time::Duration;

        ensure_test_registry();
        let reg = registry();
        let inner: Arc<dyn crate::plugins::ox_shared::registry::SharedInner> =
            Arc::new(PoolInner::new(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                4,
                Duration::from_secs(300),
            ));
        let entry = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
        let id = entry.id;
        let pool = (*inner).as_any_pool().unwrap();
        pool.bind_id(id);
        assert!(pool.try_reserve_budget());
        pool.deposit_new(crate::plugins::ox_shared::types::pool::PoolSlot::new(
            std::ptr::null_mut(),
            crate::plugins::ox_shared::types::pool::current_thread_key(),
        ));

        let v = entry_to_json(&entry);
        assert_eq!(v["type"], "Pool");
        let ts = &v["type_specific"];
        assert_eq!(ts["max_size"], 4);
        assert_eq!(ts["size"], 1);
        assert_eq!(ts["idle"], 1);
        assert_eq!(ts["in_use"], 0);
        assert_eq!(ts["waiting"], 0);
        assert_eq!(ts["rebalance_strategy"], "strict");
        // idle_by_thread must carry at least one entry for the
        // current thread's key. Keys are stringified u64s.
        let by_thread = ts["idle_by_thread"].as_object().unwrap();
        assert_eq!(by_thread.len(), 1);
        assert_eq!(by_thread.values().next().unwrap().as_u64().unwrap(), 1u64);

        drop(inner);
        drop(entry);
    }

    #[test]
    fn types_endpoint_includes_pool() {
        ensure_test_registry();
        let method = Method::GET;
        let headers = HeaderMap::new();
        let req = PluginInternalRequest {
            method: &method,
            path: "/__ox_shared/types",
            headers: &headers,
            query: None,
        };
        let resp = handle_types(&req);
        assert_eq!(resp.status().as_u16(), 200);
        let bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { resp.into_body().collect().await.unwrap().to_bytes() });
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        let types = v["types"].as_array().expect("types array");
        let p = types.iter().find(|e| e["name"] == "Pool").expect("Pool");
        assert_eq!(p["tag"], 50);
        assert_eq!(p["php_class"], "OxPHP\\Shared\\Pool");
    }

    #[test]
    fn pool_prometheus_metrics_emitted() {
        use crate::plugins::ox_shared::types::pool::PoolInner;
        use std::time::Duration;

        ensure_test_registry();
        let reg = registry();
        let inner: Arc<dyn crate::plugins::ox_shared::registry::SharedInner> =
            Arc::new(PoolInner::new(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                4,
                Duration::from_secs(300),
            ));
        let entry = reg.insert(SharedType::Pool, Arc::clone(&inner)).unwrap();
        let id = entry.id;
        let pool = (*inner).as_any_pool().unwrap();
        pool.bind_id(id);
        pool.record_acquire_ok();
        pool.record_acquire_timeout();
        pool.record_wait(std::time::Duration::from_millis(5));
        pool.record_evicted(
            2,
            crate::plugins::ox_shared::types::pool::EvictReason::Manual,
        );

        let mut output = String::new();
        SharedMetricsCollector.collect(&mut output);

        // All seven metric series per spec must appear.
        assert!(
            output.contains(&format!("oxphp_shared_pool_size{{pool_id=\"{id}\"}}")),
            "missing size gauge"
        );
        assert!(
            output.contains(&format!("oxphp_shared_pool_in_use{{pool_id=\"{id}\"}}")),
            "missing in_use gauge"
        );
        assert!(
            output.contains(&format!("oxphp_shared_pool_idle{{pool_id=\"{id}\"}}")),
            "missing idle gauge"
        );
        assert!(
            output.contains(&format!("oxphp_shared_pool_waiting{{pool_id=\"{id}\"}}")),
            "missing waiting gauge"
        );
        assert!(
            output.contains(&format!(
                "oxphp_shared_pool_acquire_total{{pool_id=\"{id}\",result=\"ok\"}} 1"
            )),
            "missing acquire ok counter"
        );
        assert!(
            output.contains(&format!(
                "oxphp_shared_pool_acquire_total{{pool_id=\"{id}\",result=\"timeout\"}} 1"
            )),
            "missing acquire timeout counter"
        );
        assert!(
            output.contains(&format!(
                "oxphp_shared_pool_wait_seconds_count{{pool_id=\"{id}\"}} 1"
            )),
            "missing wait count"
        );
        assert!(
            output.contains(&format!(
                "oxphp_shared_pool_wait_seconds_bucket{{pool_id=\"{id}\",le=\"0.01\"}} 1"
            )),
            "missing wait bucket"
        );
        assert!(
            output.contains(&format!(
                "oxphp_shared_pool_evicted_total{{pool_id=\"{id}\",reason=\"evict\"}} 2"
            )),
            "missing manual evict counter"
        );

        drop(inner);
        drop(entry);
    }

    #[test]
    fn types_endpoint_includes_map() {
        ensure_test_registry();
        let method = Method::GET;
        let headers = HeaderMap::new();
        let req = PluginInternalRequest {
            method: &method,
            path: "/__ox_shared/types",
            headers: &headers,
            query: None,
        };
        let resp = handle_types(&req);
        assert_eq!(resp.status().as_u16(), 200);
        let bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { resp.into_body().collect().await.unwrap().to_bytes() });
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        let types = v["types"].as_array().expect("types array");
        let m = types.iter().find(|e| e["name"] == "Map").expect("Map");
        assert_eq!(m["tag"], 20);
        assert_eq!(m["php_class"], "OxPHP\\Shared\\Map");
    }

    #[tokio::test]
    async fn graph_endpoint_walks_outgoing_edges() {
        use crate::plugins::ox_shared::types::counter::CounterInner;
        use crate::plugins::ox_shared::types::map::MapInner;
        use crate::plugins::ox_shared::value::{SharedRefOwned, SharedValue as SV};

        ensure_test_registry();
        let reg = registry();

        // Build: Map A -> Counter C; ask graph for root=A.
        let counter: Arc<dyn crate::plugins::ox_shared::registry::SharedInner> =
            Arc::new(CounterInner::new(0));
        let c_entry = reg.insert(SharedType::Counter, counter).unwrap();
        let c_id = c_entry.id;

        let a_inner: Arc<dyn crate::plugins::ox_shared::registry::SharedInner> =
            Arc::new(MapInner::new(None));
        let a_entry = reg.insert(SharedType::Map, Arc::clone(&a_inner)).unwrap();
        let a_id = a_entry.id;
        let a = (*a_inner).as_any_map().unwrap();
        a.bind_id(a_id);
        a.set(
            Arc::from("c"),
            SV::Shared(SharedRefOwned::from_arc(Arc::clone(&c_entry))),
        )
        .unwrap();

        let query = format!("id={a_id}");
        let method = Method::GET;
        let headers = HeaderMap::new();
        let req = PluginInternalRequest {
            method: &method,
            path: "/__ox_shared/graph",
            headers: &headers,
            query: Some(&query),
        };
        let resp = handle_graph(&req);
        assert_eq!(resp.status().as_u16(), 200);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["root"], a_id);
        let nodes = v["nodes"].as_array().unwrap();
        let edges = v["edges"].as_array().unwrap();
        assert_eq!(nodes.len(), 2, "a + c");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["from"], a_id);
        assert_eq!(edges[0]["to"], c_id);
        assert_eq!(v["truncated"], false);

        a.clear();
        drop(a_inner);
        drop(a_entry);
        drop(c_entry);
    }

    #[test]
    fn prometheus_output_includes_map_gauges() {
        use crate::plugins::ox_shared::types::map::MapInner;
        use crate::plugins::ox_shared::value::SharedValue as SV;

        ensure_test_registry();
        let reg = registry();
        let inner: Arc<dyn crate::plugins::ox_shared::registry::SharedInner> =
            Arc::new(MapInner::new(Some(10)));
        let entry = reg.insert(SharedType::Map, Arc::clone(&inner)).unwrap();
        let id = entry.id;
        let concrete = (*inner).as_any_map().unwrap();
        concrete.bind_id(id);
        concrete.set(Arc::from("a"), SV::Long(1)).unwrap();
        concrete.set(Arc::from("b"), SV::Long(2)).unwrap();

        let mut out = String::new();
        SharedMetricsCollector.collect(&mut out);

        assert!(out.contains("# TYPE oxphp_shared_map_entries gauge\n"));
        assert!(out.contains("# TYPE oxphp_shared_map_max_entries gauge\n"));
        assert!(out.contains("# TYPE oxphp_shared_map_saturation gauge\n"));
        assert!(out.contains(&format!("oxphp_shared_map_entries{{map_id=\"{id}\"}} 2\n")));
        assert!(out.contains(&format!(
            "oxphp_shared_map_max_entries{{map_id=\"{id}\"}} 10\n"
        )));
        // 2/10 = 0.2000
        assert!(out.contains(&format!(
            "oxphp_shared_map_saturation{{map_id=\"{id}\"}} 0.2000\n"
        )));

        drop(inner);
        drop(entry);
    }

    #[test]
    fn prometheus_output_includes_channel_gauges() {
        ensure_test_registry();
        let reg = registry();
        let a = Arc::new(ChannelInner::new(4));
        let b = Arc::new(ChannelInner::new(8));
        // Deposit a few payloads so `pending` > 0 for one of them.
        a.try_send(b"x".to_vec()).expect("send a");
        a.try_send(b"y".to_vec()).expect("send a2");

        let entry_a = reg
            .insert(SharedType::Channel, a.clone())
            .expect("insert a");
        let entry_b = reg
            .insert(SharedType::Channel, b.clone())
            .expect("insert b");
        let id_a = entry_a.id;
        let id_b = entry_b.id;

        let mut out = String::new();
        SharedMetricsCollector.collect(&mut out);

        // HELP/TYPE lines emitted exactly once regardless of channel count.
        assert!(out.contains("# TYPE oxphp_shared_channel_pending gauge\n"));
        assert!(out.contains("# TYPE oxphp_shared_channel_senders_blocked gauge\n"));
        assert!(out.contains("# TYPE oxphp_shared_channel_receivers_blocked gauge\n"));
        assert!(out.contains("# TYPE oxphp_shared_channel_items_sent_total counter\n"));
        assert!(out.contains("# TYPE oxphp_shared_channel_items_dropped_total counter\n"));

        // Per-id series present for both channels.
        let needle_a = format!("oxphp_shared_channel_pending{{channel_id=\"{id_a}\"}} 2\n");
        let needle_b = format!("oxphp_shared_channel_pending{{channel_id=\"{id_b}\"}} 0\n");
        assert!(out.contains(&needle_a), "expected {needle_a:?} in\n{out}");
        assert!(out.contains(&needle_b), "expected {needle_b:?} in\n{out}");
        // items_sent_total reflects the two successful sends on channel A.
        let sent_a = format!("oxphp_shared_channel_items_sent_total{{channel_id=\"{id_a}\"}} 2\n");
        assert!(out.contains(&sent_a), "expected {sent_a:?} in\n{out}");

        drop(entry_a);
        drop(entry_b);
    }
}
