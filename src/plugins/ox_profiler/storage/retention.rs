//! Retention background task. Trims `index.json` to the most
//! recent N entries and deletes the format files for trimmed runs.
//! Runs every `RETENTION_INTERVAL`; non-blocking; bounded work
//! per pass.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::RunMeta;

const RETENTION_INTERVAL: Duration = Duration::from_secs(5);

/// Spawn the retention task on the current Tokio runtime.
/// Returns immediately; the task runs forever (or until the
/// runtime shuts down).
///
/// `index_lock` must be the same `Arc` given to the
/// paired `DiskWriter`; it serialises the sweep's read-then-
/// rewrite-via-rename against append and DELETE.
///
/// Safely no-ops (with a debug log) when called outside a Tokio
/// context — happens in unit tests that exercise plugin init
/// without a runtime; production always has one (the executor's
/// Tokio runtime is up before plugin init completes in `main.rs`).
pub fn spawn(output_dir: PathBuf, retention_count: usize, index_lock: Arc<tokio::sync::Mutex<()>>) {
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::debug!(
            plugin = "profiler",
            "retention task not spawned: no Tokio runtime in context"
        );
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RETENTION_INTERVAL).await;
            if let Err(e) = sweep_once(&output_dir, retention_count, &index_lock).await {
                tracing::warn!(
                    plugin = "profiler",
                    output_dir = %output_dir.display(),
                    error = %e,
                    "retention sweep failed (will retry next interval)"
                );
            }
        }
    });
}

async fn sweep_once(
    output_dir: &Path,
    retention_count: usize,
    index_lock: &tokio::sync::Mutex<()>,
) -> std::io::Result<()> {
    let index_path = output_dir.join("index.json");
    if !index_path.exists() {
        return Ok(());
    }

    // Hold the mutex across the read → sort → write-tmp → rename
    // sequence so concurrent appends/DELETEs can't interleave.
    let _guard = index_lock.lock().await;

    // Read all entries.
    let f = File::open(&index_path).await?;
    let mut reader = BufReader::new(f).lines();
    let mut entries: Vec<RunMeta> = Vec::new();
    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<RunMeta>(&line) {
            Ok(meta) => entries.push(meta),
            Err(_) => continue, // skip malformed
        }
    }

    if entries.len() <= retention_count {
        return Ok(());
    }

    // Sort newest first; keep the head, drop the tail.
    entries.sort_by_key(|e| std::cmp::Reverse(e.timestamp_ms));
    let drop_tail: Vec<RunMeta> = entries.split_off(retention_count);

    // Rewrite index.json atomically — write tmp, rename over.
    let tmp_path = output_dir.join("index.json.tmp");
    {
        let mut tmp = File::create(&tmp_path).await?;
        for keep in &entries {
            let mut line = serde_json::to_vec(keep).expect("RunMeta serialise");
            line.push(b'\n');
            tmp.write_all(&line).await?;
        }
        tmp.flush().await?;
    }
    fs::rename(&tmp_path, &index_path).await?;

    // Delete the format files for dropped entries. Errors logged
    // at DEBUG and ignored — orphan files become eventually-
    // consistent garbage that a future sweep extension could clean
    // up via an orphan walk (deferred).
    for dropped in &drop_tail {
        for fmt_ext in &dropped.formats {
            let path = output_dir.join(format!("{}.{}", dropped.run_id, fmt_ext));
            if let Err(e) = fs::remove_file(&path).await {
                tracing::debug!(
                    plugin = "profiler",
                    path = %path.display(),
                    error = %e,
                    "retention drop: file remove failed (likely already gone)"
                );
            }
        }
    }

    tracing::debug!(
        plugin = "profiler",
        kept = entries.len(),
        dropped = drop_tail.len(),
        "retention sweep complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_profiler::trigger::ActivationSource;
    use tempfile::TempDir;

    fn meta(run_id: &str, ts: u64) -> RunMeta {
        RunMeta {
            run_id: run_id.into(),
            request_id: "r".into(),
            trace_id: None,
            timestamp_ms: ts,
            duration_ms: 0,
            method: "GET".into(),
            url: "/".into(),
            status: 200,
            user_agent: None,
            client_ip: None,
            source: ActivationSource::Header,
            span_count: 0,
            event_count: 0,
            error_count: 0,
            leaked_count: 0,
            truncated: false,
            oxphp_version: "0.2.0".into(),
            formats: vec!["collapsed".into()],
        }
    }

    async fn write_index(dir: &Path, entries: &[RunMeta]) {
        let path = dir.join("index.json");
        let mut f = File::create(&path).await.unwrap();
        for e in entries {
            let mut line = serde_json::to_vec(e).unwrap();
            line.push(b'\n');
            f.write_all(&line).await.unwrap();
        }
        f.flush().await.unwrap();
    }

    #[tokio::test]
    async fn sweep_under_cap_is_noop() {
        let dir = TempDir::new().unwrap();
        write_index(
            dir.path(),
            &[meta("a", 100), meta("b", 200), meta("c", 300)],
        )
        .await;
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        sweep_once(dir.path(), 5, &lock).await.unwrap();
        let lines = std::fs::read_to_string(dir.path().join("index.json")).unwrap();
        assert_eq!(lines.lines().count(), 3, "all kept");
    }

    #[tokio::test]
    async fn sweep_drops_oldest_past_cap() {
        let dir = TempDir::new().unwrap();
        // Three entries, cap=2 → oldest ("a", ts=100) dropped.
        write_index(
            dir.path(),
            &[meta("a", 100), meta("b", 200), meta("c", 300)],
        )
        .await;
        // Create the file we expect retention to delete.
        std::fs::write(dir.path().join("a.collapsed"), b"x").unwrap();
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        sweep_once(dir.path(), 2, &lock).await.unwrap();
        let body = std::fs::read_to_string(dir.path().join("index.json")).unwrap();
        assert!(body.contains("\"c\""));
        assert!(body.contains("\"b\""));
        assert!(!body.contains("\"a\""), "oldest dropped");
        assert!(
            !dir.path().join("a.collapsed").exists(),
            "format file deleted"
        );
    }
}
