//! XHProf JSON exporter — flat pair map + optional xhgui envelope.
//!
//! Output is xhgui's `/run/import` import format. Compatible with
//! the upstream xhgui parser used by Tideways, Splat, and other
//! XHProf-derived UIs.

use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::profiling::SpanTree;

/// Output mode for `export_xhprof`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhprofMode {
    /// Bare pair-map: `{"a==>b": {ct, wt, cpu, mu, pmu}, ...}`.
    Raw,
    /// `{"profile": <pair_map>, "meta": <XhguiMeta>}` — xhgui-import
    /// compatible.
    Xhgui,
}

/// Request-context fields the xhgui envelope expects. Storage
/// fills these from the `PluginCompleteView` at export time.
#[derive(Debug, Clone, Default)]
pub struct XhguiMeta {
    /// Combined `METHOD URL` for the `simple_url` field — xhgui's
    /// primary group key. Leave empty for non-HTTP requests.
    pub url: String,
    /// HTTP method as the `request_method` field.
    pub request_method: String,
    /// Unix epoch timestamp (seconds) at storage time (request completion).
    pub request_ts: u64,
    /// Same, with microsecond precision (xhgui likes both).
    pub request_ts_micro: f64,
    /// Server name / hostname for the `SERVER` envelope.
    pub server_name: String,
    /// Optional structured GET / POST / SERVER hash maps. Empty by
    /// default; storage may populate from the request view.
    pub get: Map<String, Value>,
    pub post: Map<String, Value>,
    pub server: Map<String, Value>,
}

#[derive(Debug, Default, Clone, Copy)]
struct PairAccum {
    ct: u64,
    wt: u64,
    cpu: u64,
    mu: i64,
    pmu: i64,
}

impl PairAccum {
    fn add(&mut self, span_wt: u64, span_cpu: u64, span_mu: i64, span_pmu: i64) {
        self.ct = self.ct.saturating_add(1);
        self.wt = self.wt.saturating_add(span_wt);
        self.cpu = self.cpu.saturating_add(span_cpu);
        self.mu = self.mu.saturating_add(span_mu.max(0));
        self.pmu = self.pmu.saturating_add(span_pmu.max(0));
    }
}

/// Build the XHProf pair-map shared by every envelope: the bare
/// `{"caller==>callee": {ct,wt,cpu,mu,pmu}}` object plus the synthetic
/// `main()` self-entry. Each envelope (raw / xhgui / buggregator) wraps
/// this same value.
fn build_profile_value(tree: &SpanTree) -> Value {
    let mut pair_map: HashMap<String, PairAccum> = HashMap::new();

    // Build span_id → &FinishedSpan once.
    let mut by_id: HashMap<&str, &crate::profiling::FinishedSpan> =
        HashMap::with_capacity(tree.finished.len());
    for s in &tree.finished {
        by_id.insert(s.span_id.as_ref(), s);
    }

    for span in &tree.finished {
        let parent_name: &str = if span.parent_span_id == tree.root_span_id {
            "main()"
        } else {
            by_id
                .get(span.parent_span_id.as_ref())
                .map(|p| p.name.as_ref())
                .unwrap_or("main()") // orphan → re-parent to main()
        };
        let key = format!("{parent_name}==>{}", span.name);

        let wt = span.end_ns.saturating_sub(span.start_ns) / 1000;
        let cpu = span.cpu_ns / 1000;
        let mu = span.mem_exit - span.mem_enter;
        let pmu = span.mem_peak - span.mem_enter;

        pair_map.entry(key).or_default().add(wt, cpu, mu, pmu);
    }

    // Synthetic main() self-entry: aggregates all root-level spans.
    // xhgui needs this so the root row in the UI has a value.
    let mut main_self = PairAccum::default();
    for span in &tree.finished {
        if span.parent_span_id == tree.root_span_id {
            let wt = span.end_ns.saturating_sub(span.start_ns) / 1000;
            let cpu = span.cpu_ns / 1000;
            let mu = span.mem_exit - span.mem_enter;
            let pmu = span.mem_peak - span.mem_enter;
            main_self.add(wt, cpu, mu, pmu);
        }
    }
    if main_self.ct > 0 {
        pair_map.insert("main()".to_string(), main_self);
    }

    // Serialise the pair map. Sort keys for deterministic output —
    // makes golden fixtures stable.
    let mut sorted_keys: Vec<&String> = pair_map.keys().collect();
    sorted_keys.sort();
    let mut profile = Map::new();
    for k in sorted_keys {
        let p = pair_map.get(k).unwrap();
        profile.insert(
            k.clone(),
            json!({
                "ct": p.ct,
                "wt": p.wt,
                "cpu": p.cpu,
                "mu": p.mu,
                "pmu": p.pmu,
            }),
        );
    }
    Value::Object(profile)
}

/// Render the tree as XHProf JSON. Returns UTF-8 bytes (compact JSON,
/// no trailing newline).
pub fn export_xhprof(tree: &SpanTree, mode: XhprofMode, meta: Option<XhguiMeta>) -> Vec<u8> {
    let profile_value = build_profile_value(tree);

    let final_value = match mode {
        XhprofMode::Raw => profile_value,
        XhprofMode::Xhgui => {
            let meta = meta.unwrap_or_default();
            let mut envelope = Map::new();
            envelope.insert("profile".into(), profile_value);
            envelope.insert(
                "meta".into(),
                json!({
                    "url": meta.url,
                    "simple_url": meta.url,
                    "request_method": meta.request_method,
                    "request_ts": meta.request_ts,
                    "request_ts_micro": meta.request_ts_micro,
                    "request_date": "",   // optional; storage may set
                    "SERVER": meta.server,
                    "GET": meta.get,
                    "POST": meta.post,
                    "server_name": meta.server_name,
                }),
            );
            Value::Object(envelope)
        }
    };

    serde_json::to_vec(&final_value).expect("xhprof JSON serialisation should not fail")
}

/// App/host context for the Buggregator profiler envelope, everything the
/// envelope needs except the per-request `date`. Built once from the profiler
/// config (`app_name`, `tags`) and the host (`hostname`), then reused across
/// pushes — [`export_xhprof_buggregator`] takes it by reference so no per-push
/// allocation of the tags or strings is needed. `tags` are ordered pairs and
/// serialize to a JSON object preserving that order.
#[derive(Debug, Clone, Default)]
pub struct BuggregatorMeta {
    pub app_name: String,
    pub tags: Vec<(String, String)>,
    pub hostname: String,
}

/// Serializes ordered `(key, value)` pairs as a JSON object, preserving
/// insertion order (unlike `serde_json::Map`, which is a `BTreeMap` and would
/// re-sort keys alphabetically without the `preserve_order` feature).
struct OrderedTags<'a>(&'a [(String, String)]);

impl serde::Serialize for OrderedTags<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// Render the tree in Buggregator's `/api/profiler/store` envelope:
/// `{profile, tags, app_name, hostname, date}`. The `profile` value is
/// the same bare pair-map raw/xhgui emit; Buggregator parses it into its
/// own edge model server-side, and reads `app_name`/`tags` for project
/// grouping and filtering. `date` is the run's Unix epoch seconds, passed
/// per call; `meta` (app_name/tags/hostname) is borrowed from a shared
/// template.
pub fn export_xhprof_buggregator(tree: &SpanTree, meta: &BuggregatorMeta, date: u64) -> Vec<u8> {
    // Borrowed serialization so a hot-path push clones nothing but the
    // profile it must build anyway.
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        profile: Value,
        tags: OrderedTags<'a>,
        app_name: &'a str,
        hostname: &'a str,
        date: u64,
    }
    let envelope = Envelope {
        profile: build_profile_value(tree),
        tags: OrderedTags(&meta.tags),
        app_name: &meta.app_name,
        hostname: &meta.hostname,
        date,
    };
    serde_json::to_vec(&envelope).expect("xhprof buggregator JSON serialisation should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiling::{FinishedSpan, ProfilingMode};
    use std::sync::Arc;

    fn mk_span(name: &str, span_id: &str, parent: &str, wt_us: u64) -> FinishedSpan {
        FinishedSpan {
            local_id: 0,
            trace_id: Arc::<str>::from(""),
            span_id: Arc::<str>::from(span_id),
            parent_span_id: Arc::<str>::from(parent),
            name: Arc::<str>::from(name),
            start_ns: 0,
            end_ns: wt_us * 1000,
            attributes: Vec::new(),
            events: Vec::new(),
            status_code: 0,
            status_message: None,
            leaked: false,
            cpu_ns: 0,
            mem_enter: 0,
            mem_exit: 0,
            mem_peak: 0,
        }
    }

    /// Root is "root"; a single child re-parents to `main()`.
    fn sample_tree() -> SpanTree {
        SpanTree {
            mode: ProfilingMode::ProfileAll,
            trace_id: Arc::<str>::from(""),
            root_span_id: Arc::<str>::from("root"),
            finished: vec![mk_span("Foo::bar", "s1", "root", 450)],
        }
    }

    #[test]
    fn buggregator_envelope_has_exact_top_level_keys() {
        let bytes = export_xhprof_buggregator(&sample_tree(), &BuggregatorMeta::default(), 0);
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        keys.sort();
        // The five keys spiral-packages/profiler's WebStorage POSTs.
        assert_eq!(
            keys,
            vec!["app_name", "date", "hostname", "profile", "tags"]
        );
    }

    #[test]
    fn buggregator_profile_is_the_same_bare_pairmap_as_raw() {
        let tree = sample_tree();
        let raw = export_xhprof(&tree, XhprofMode::Raw, None);
        let raw_v: Value = serde_json::from_slice(&raw).unwrap();

        let bugg = export_xhprof_buggregator(&tree, &BuggregatorMeta::default(), 0);
        let bugg_v: Value = serde_json::from_slice(&bugg).unwrap();

        // Buggregator wraps the identical pair-map raw emits under `profile`.
        assert_eq!(bugg_v["profile"], raw_v);
        // Sanity: the pair-map carries the child arc + the synthetic main().
        assert!(bugg_v["profile"].get("main()==>Foo::bar").is_some());
        assert!(bugg_v["profile"].get("main()").is_some());
    }

    #[test]
    fn buggregator_meta_fields_serialise_with_correct_types() {
        let meta = BuggregatorMeta {
            app_name: "shop".into(),
            tags: vec![("env".into(), "prod".into())],
            hostname: "web-1".into(),
        };
        let v: Value = serde_json::from_slice(&export_xhprof_buggregator(
            &sample_tree(),
            &meta,
            1_700_000_000,
        ))
        .unwrap();
        assert_eq!(v["app_name"], Value::String("shop".into()));
        assert_eq!(v["hostname"], Value::String("web-1".into()));
        assert_eq!(v["date"], Value::from(1_700_000_000u64)); // JSON number, not string
        assert_eq!(v["tags"]["env"], Value::String("prod".into()));
    }

    #[test]
    fn buggregator_tags_preserve_insertion_order_on_the_wire() {
        // Keys ordered so insertion order (z_env, a_tier, m_region) differs
        // from alphabetical — proves no BTreeMap re-sort on the wire.
        let meta = BuggregatorMeta {
            app_name: String::new(),
            tags: vec![
                ("z_env".into(), "prod".into()),
                ("a_tier".into(), "web".into()),
                ("m_region".into(), "eu".into()),
            ],
            hostname: String::new(),
        };
        let s = String::from_utf8(export_xhprof_buggregator(&sample_tree(), &meta, 0)).unwrap();
        let z = s.find("z_env").unwrap();
        let a = s.find("a_tier").unwrap();
        let m = s.find("m_region").unwrap();
        assert!(z < a && a < m, "tags reordered on the wire: {s}");
    }

    #[test]
    fn buggregator_default_meta_yields_empty_strings_and_object() {
        let v: Value = serde_json::from_slice(&export_xhprof_buggregator(
            &sample_tree(),
            &BuggregatorMeta::default(),
            0,
        ))
        .unwrap();
        assert_eq!(v["app_name"], Value::String(String::new()));
        assert_eq!(v["hostname"], Value::String(String::new()));
        assert_eq!(v["date"], Value::from(0u64));
        assert!(v["tags"].as_object().unwrap().is_empty());
    }
}
