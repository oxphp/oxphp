//! Disk spooling for buffered PHP response bodies.
//!
//! A buffered (non-streaming) response is sent when the script ends or calls
//! `oxphp_finish_request()`, so everything the script writes is held until
//! that point — and, as the body, for as long afterwards as the client takes
//! to read it. Held in memory, outside PHP's `memory_limit`, a 500 MiB
//! `readfile()` costs 500 MiB of RSS for the whole download. Past
//! `RESPONSE_BUFFER_BYTES` the output moves to a temporary file instead,
//! created in the system temporary directory (`TMPDIR`) and unlinked at once,
//! and the body is served from that file — the way nginx saves a FastCGI
//! response that does not fit its buffers to a temporary file. Output still
//! in memory when the response is sent goes to the end of the file, so a
//! spooled body stays out of memory while its client reads it — unless it is
//! eligible for compression, which reads a body of at most 3 MiB back in.
//!
//! Every spool file of the process draws on one budget,
//! `RESPONSE_SPOOL_MAX_BYTES`. A response that cannot be spooled — the budget
//! is spent, or the file cannot be created or written — is refused: the rest
//! of its output is discarded, the script runs to completion, and the client
//! gets `503` with `Retry-After`.
//!
//! Streamed output (`text/event-stream`, `oxphp_stream_flush()`) leaves at
//! every flush and is never spooled; neither is output written on an async
//! task thread, which has no client to send it to.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::task::{ready, Context, Poll};

use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Body, Frame, SizeHint};
use tokio::io::{AsyncRead, ReadBuf};

use crate::types::{full_body, stream_body, ResponseBody};

/// Output a response holds in memory before it is spooled (`RESPONSE_BUFFER_BYTES`).
pub const DEFAULT_BUFFER_BYTES: usize = 1 << 20;

/// What all spool files of the process may hold together (`RESPONSE_SPOOL_MAX_BYTES`).
pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;

/// Spool space shared by every response of the process.
pub struct SpoolBudget {
    in_use: AtomicU64,
    max: u64,
}

impl SpoolBudget {
    pub const fn new(max: u64) -> Self {
        Self {
            in_use: AtomicU64::new(0),
            max,
        }
    }

    /// Bytes the live spool files hold between them.
    pub fn in_use(&self) -> u64 {
        self.in_use.load(Ordering::Relaxed)
    }

    fn reserve(&self, bytes: u64) -> bool {
        self.in_use
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|&total| total <= self.max)
            })
            .is_ok()
    }

    fn release(&self, bytes: u64) {
        self.in_use.fetch_sub(bytes, Ordering::Relaxed);
    }
}

/// When output is spooled and where to.
pub struct SpoolConfig {
    /// Output a response holds in memory before it is spooled.
    pub buffer_bytes: usize,
    /// Directory spool files are created (and at once unlinked) in.
    pub dir: PathBuf,
    pub budget: &'static SpoolBudget,
}

impl SpoolConfig {
    pub fn new(buffer_bytes: usize, max_bytes: u64, dir: PathBuf) -> Self {
        Self {
            buffer_bytes,
            dir,
            budget: Box::leak(Box::new(SpoolBudget::new(max_bytes))),
        }
    }
}

static CONFIG: OnceLock<SpoolConfig> = OnceLock::new();

/// Install the process-wide configuration, with spool files in the system
/// temporary directory. Only the first call has an effect, so it has to come
/// before the first request.
pub fn configure(buffer_bytes: usize, max_bytes: u64) -> &'static SpoolConfig {
    CONFIG.get_or_init(|| {
        SpoolConfig::new(
            buffer_bytes,
            max_bytes,
            spool_dir(std::env::var_os("TMPDIR")),
        )
    })
}

/// `TMPDIR`, or `/tmp`. An empty `TMPDIR` counts as unset, as it does for
/// PHP; `std::env::temp_dir()` would return it as an empty path, which puts
/// the files in the working directory.
fn spool_dir(tmpdir: Option<OsString>) -> PathBuf {
    match tmpdir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from("/tmp"),
    }
}

/// The process-wide configuration — the defaults if `configure` never ran.
pub fn config() -> &'static SpoolConfig {
    configure(DEFAULT_BUFFER_BYTES, DEFAULT_MAX_BYTES)
}

/// Check that a spool file can be created where `config` puts them.
pub fn probe_dir(config: &SpoolConfig) -> io::Result<()> {
    create_unlinked(&config.dir).map(drop)
}

/// A spool file's share of the budget, given back on drop.
struct Reservation {
    budget: &'static SpoolBudget,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

/// The start of a response's output, moved out of memory: an unlinked file
/// and the budget its bytes hold.
pub struct Spool {
    file: File,
    reservation: Reservation,
}

enum SpillError {
    Full,
    Io(io::Error),
}

impl From<io::Error> for SpillError {
    fn from(err: io::Error) -> Self {
        SpillError::Io(err)
    }
}

impl Spool {
    fn create(config: &SpoolConfig) -> io::Result<Self> {
        Ok(Self {
            file: create_unlinked(&config.dir)?,
            reservation: Reservation {
                budget: config.budget,
                bytes: 0,
            },
        })
    }

    fn write(&mut self, head: &[u8], data: &[u8]) -> Result<(), SpillError> {
        let bytes = (head.len() + data.len()) as u64;
        if !self.reservation.budget.reserve(bytes) {
            return Err(SpillError::Full);
        }
        self.reservation.bytes += bytes;
        self.file.write_all(head)?;
        self.file.write_all(data)?;
        Ok(())
    }
}

/// Where a buffered response's output is going.
#[derive(Default)]
pub enum Spooled {
    /// All of it is in the in-memory buffer.
    #[default]
    InMemory,
    /// It starts in this file; the in-memory buffer holds what came after.
    Spooling(Box<Spool>),
    /// It could not be spooled. The rest is discarded and the response is
    /// answered with [`refusal`].
    Refused,
}

impl Spooled {
    /// Add `data` to the response, whose in-memory buffer is `output`.
    ///
    /// Appends to `output` while `output` and `data` together fit in
    /// `config.buffer_bytes`, or when `spoolable` — asked only then — says
    /// this output has to stay in memory. Otherwise both go to the spool file
    /// and `output` is emptied, so a single large write never passes through
    /// memory. If that fails, the output so far is dropped and the response
    /// is refused.
    pub fn append(
        &mut self,
        output: &mut Vec<u8>,
        data: &[u8],
        config: &SpoolConfig,
        spoolable: impl FnOnce() -> bool,
    ) {
        if matches!(self, Spooled::Refused) {
            return;
        }
        if output.len() + data.len() <= config.buffer_bytes || !spoolable() {
            output.extend_from_slice(data);
            return;
        }
        self.spill_or_refuse(output, data, config);
    }

    /// Write `output`, then `data`, to the spool file and empty `output`. If
    /// that fails, the output so far is dropped and the response is refused.
    fn spill_or_refuse(&mut self, output: &mut Vec<u8>, data: &[u8], config: &SpoolConfig) {
        if let Err(err) = self.spill(output, data, config) {
            match err {
                SpillError::Full => tracing::warn!(
                    max_bytes = config.budget.max,
                    "Response body refused with 503: the response spool is full \
                     (RESPONSE_SPOOL_MAX_BYTES)"
                ),
                SpillError::Io(error) => tracing::warn!(
                    dir = %config.dir.display(),
                    %error,
                    "Response body refused with 503: cannot spool it to disk"
                ),
            }
            *self = Spooled::Refused;
        }
        output.clear();
    }

    fn spill(
        &mut self,
        output: &[u8],
        data: &[u8],
        config: &SpoolConfig,
    ) -> Result<(), SpillError> {
        if matches!(self, Spooled::InMemory) {
            *self = Spooled::Spooling(Box::new(Spool::create(config)?));
        }
        if let Spooled::Spooling(spool) = self {
            spool.write(output, data)?;
        }
        Ok(())
    }

    /// Move the spool file out, to become the response body. What `output`
    /// still holds is written to the end of the file first, so the body
    /// leaves nothing in the in-memory buffer. A refusal —
    /// one that writing it causes, too — stays, so output the script writes
    /// afterwards, a stream it starts, is still discarded.
    pub fn take(&mut self, output: &mut Vec<u8>, config: &SpoolConfig) -> Option<Box<Spool>> {
        if matches!(self, Spooled::Spooling(_)) && !output.is_empty() {
            self.spill_or_refuse(output, &[], config);
        }
        match std::mem::take(self) {
            Spooled::Spooling(spool) => Some(spool),
            Spooled::Refused => {
                *self = Spooled::Refused;
                None
            }
            Spooled::InMemory => None,
        }
    }

    pub fn is_refused(&self) -> bool {
        matches!(self, Spooled::Refused)
    }
}

/// Body, headers and status that answer a refused response. The script's
/// own headers go with its output: they describe a body that is not there.
pub fn refusal() -> (Vec<u8>, Vec<(String, String)>, u16) {
    (
        b"503 Service Unavailable".to_vec(),
        vec![
            ("Content-Type".into(), "text/plain; charset=utf-8".into()),
            ("Retry-After".into(), "5".into()),
        ],
        503,
    )
}

/// The response body: the spool file, if there is one, then `tail` — the
/// output kept in memory, which [`Spooled::take`] leaves empty when there is a
/// spool — then, for a stream, the chunks arriving on `rx`.
pub fn body(
    spool: Option<Box<Spool>>,
    tail: Bytes,
    rx: Option<tokio::sync::mpsc::Receiver<Bytes>>,
) -> ResponseBody {
    let Some(spool) = spool else {
        return match rx {
            Some(rx) => stream_body(tail, rx),
            None => full_body(tail),
        };
    };
    let Spool {
        mut file,
        reservation,
    } = *spool;
    let spooled = reservation.bytes;
    // Unless a stream follows, the length is known up front, so the response
    // keeps the Content-Length an in-memory body gets.
    let remaining = rx.is_none().then(|| spooled + tail.len() as u64);
    let rewound = file.rewind();
    let file = SpoolFile {
        file: Some((
            tokio::io::AsyncReadExt::take(tokio::fs::File::from_std(file), spooled),
            reservation,
        )),
    };
    let frames = futures_util::stream::iter(rewound.err().map(Err::<Bytes, io::Error>))
        .chain(tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024))
        .chain(futures_util::stream::iter(
            (!tail.is_empty()).then_some(Ok(tail)),
        ))
        .chain(
            futures_util::stream::iter(rx)
                .flat_map(tokio_stream::wrappers::ReceiverStream::new)
                .map(Ok),
        )
        .map(|chunk| chunk.map(Frame::data));
    SpooledBody {
        inner: BodyExt::boxed(StreamBody::new(frames)),
        remaining,
    }
    .boxed()
}

/// The spool file as the body reads it, with the file's share of the budget.
/// `ReaderStream` drops it once it has read the file to its end, and the body
/// drops it if the client goes first. The drop is handed to the blocking pool
/// — `tokio::fs::File` closes the file wherever it is dropped, and closing the
/// last handle of an unlinked file frees its blocks — and the file's share of
/// the budget goes back with it.
struct SpoolFile {
    file: Option<(tokio::io::Take<tokio::fs::File>, Reservation)>,
}

impl AsyncRead for SpoolFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.file.as_mut() {
            Some((file, _)) => Pin::new(file).poll_read(cx, buf),
            None => Poll::Ready(Ok(())),
        }
    }
}

impl Drop for SpoolFile {
    fn drop(&mut self) {
        if let (Some(file), Ok(runtime)) = (self.file.take(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn_blocking(move || drop(file));
        }
    }
}

/// A body read from a spool file. Unless a stream follows the file, it
/// reports the exact length left, as an in-memory body does.
struct SpooledBody {
    inner: ResponseBody,
    remaining: Option<u64>,
}

impl Body for SpooledBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        let frame = ready!(Pin::new(&mut self.inner).poll_frame(cx));
        if let (Some(Ok(frame)), Some(remaining)) = (&frame, self.remaining.as_mut()) {
            if let Some(data) = frame.data_ref() {
                *remaining = remaining.saturating_sub(data.len() as u64);
            }
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        match self.remaining {
            Some(remaining) => SizeHint::with_exact(remaining),
            None => self.inner.size_hint(),
        }
    }
}

/// Create a file in `dir` that, once this returns, only the returned handle
/// reaches: opened under a fresh name and unlinked at once, so its space goes
/// back to the filesystem when the handle closes, crash or not.
fn create_unlinked(dir: &Path) -> io::Result<File> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    // Another process can hold a name for the moment before it unlinks it —
    // a second server with the same PID, in another container sharing the
    // directory. A fresh number gets past it.
    for _ in 0..8 {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("oxphp-response-{pid}-{seq}"));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                std::fs::remove_file(&path)?;
                return Ok(file);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free spool file name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(buffer_bytes: usize, max_bytes: u64) -> (SpoolConfig, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (
            SpoolConfig::new(buffer_bytes, max_bytes, dir.path().to_path_buf()),
            dir,
        )
    }

    fn append(spooled: &mut Spooled, output: &mut Vec<u8>, data: &[u8], config: &SpoolConfig) {
        spooled.append(output, data, config, || true);
    }

    async fn collect(body: ResponseBody) -> Vec<u8> {
        body.collect().await.unwrap().to_bytes().to_vec()
    }

    /// The budget in use once it reaches zero, or after about a second: the
    /// body gives its share back from the blocking pool.
    async fn settled_in_use(budget: &SpoolBudget) -> u64 {
        for _ in 0..1000 {
            if budget.in_use() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        budget.in_use()
    }

    #[test]
    fn output_within_the_buffer_stays_in_memory() {
        let (config, dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"abcd", &config);
        append(&mut spooled, &mut output, b"efgh", &config);

        assert!(matches!(spooled, Spooled::InMemory));
        assert_eq!(output, b"abcdefgh");
        assert_eq!(config.budget.in_use(), 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        assert!(spooled.take(&mut output, &config).is_none());
        assert_eq!(output, b"abcdefgh");
    }

    #[test]
    fn crossing_the_buffer_moves_everything_to_the_spool() {
        let (config, dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"abcdef", &config);
        append(&mut spooled, &mut output, b"ghij", &config);

        assert!(matches!(spooled, Spooled::Spooling(_)));
        assert!(output.is_empty());
        assert_eq!(config.budget.in_use(), 10);
        // Unlinked: nothing is left in the directory while the file is open.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // Later writes are buffered again until they cross the buffer.
        append(&mut spooled, &mut output, b"kl", &config);
        assert_eq!(output, b"kl");
        assert_eq!(config.budget.in_use(), 10);
    }

    #[test]
    fn a_single_large_write_bypasses_the_buffer() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, &[7u8; 100], &config);

        assert!(output.is_empty());
        assert_eq!(output.capacity(), 0);
        assert_eq!(config.budget.in_use(), 100);
    }

    #[test]
    fn output_that_must_stay_in_memory_is_not_spooled() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        spooled.append(&mut output, &[1u8; 100], &config, || false);

        assert!(matches!(spooled, Spooled::InMemory));
        assert_eq!(output.len(), 100);
        assert_eq!(config.budget.in_use(), 0);
    }

    #[test]
    fn a_full_budget_refuses_and_releases_what_was_reserved() {
        let (config, _dir) = test_config(8, 20);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, &[1u8; 12], &config);
        assert_eq!(config.budget.in_use(), 12);

        append(&mut spooled, &mut output, &[2u8; 12], &config);
        assert!(spooled.is_refused());
        assert!(output.is_empty());
        assert_eq!(config.budget.in_use(), 0);

        // Refused for good: nothing more is kept, and take() leaves it refused.
        append(&mut spooled, &mut output, b"x", &config);
        assert!(output.is_empty());
        assert!(spooled.take(&mut output, &config).is_none());
        assert!(spooled.is_refused());
    }

    #[test]
    fn taking_the_spool_moves_the_buffered_output_into_it() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"0123456789", &config);
        append(&mut spooled, &mut output, b"ab", &config);
        assert_eq!(output, b"ab");

        let spool = spooled.take(&mut output, &config).unwrap();
        assert!(output.is_empty());
        assert_eq!(spool.reservation.bytes, 12);
        assert_eq!(config.budget.in_use(), 12);
    }

    #[test]
    fn buffered_output_that_does_not_fit_refuses_at_take() {
        let (config, _dir) = test_config(8, 11);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"0123456789", &config);
        append(&mut spooled, &mut output, b"ab", &config);

        assert!(spooled.take(&mut output, &config).is_none());
        assert!(spooled.is_refused());
        assert!(output.is_empty());
        assert_eq!(config.budget.in_use(), 0);
    }

    #[test]
    fn the_budget_admits_exactly_its_maximum() {
        let (config, _dir) = test_config(8, 20);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, &[1u8; 20], &config);

        assert!(matches!(spooled, Spooled::Spooling(_)));
        assert_eq!(config.budget.in_use(), 20);
    }

    #[test]
    fn concurrent_spools_share_the_budget() {
        let (config, _dir) = test_config(8, 30);
        let (mut a, mut b) = (Spooled::default(), Spooled::default());
        let mut output = Vec::new();
        append(&mut a, &mut output, &[1u8; 20], &config);
        append(&mut b, &mut output, &[2u8; 20], &config);

        assert!(matches!(a, Spooled::Spooling(_)));
        assert!(b.is_refused());
        drop(a);
        assert_eq!(config.budget.in_use(), 0);
    }

    #[test]
    fn an_unwritable_directory_refuses() {
        let config = SpoolConfig::new(8, 1024, PathBuf::from("/nonexistent/oxphp-spool"));
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, &[1u8; 20], &config);

        assert!(spooled.is_refused());
        assert!(output.is_empty());
        assert_eq!(config.budget.in_use(), 0);
        assert!(probe_dir(&config).is_err());
    }

    #[tokio::test]
    async fn body_serves_the_spool_and_the_buffered_rest_with_an_exact_length() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"0123456789", &config);
        append(&mut spooled, &mut output, b"ab", &config);

        let spool = spooled.take(&mut output, &config);
        let body = body(spool, Bytes::from(output), None);
        assert_eq!(body.size_hint().exact(), Some(12));
        assert_eq!(collect(body).await, b"0123456789ab");
        assert_eq!(settled_in_use(config.budget).await, 0);
    }

    #[tokio::test]
    async fn body_serves_a_tail_left_in_memory_after_the_spool() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"0123456789", &config);
        let spool = spooled.take(&mut output, &config);

        let body = body(spool, Bytes::from_static(b"ab"), None);
        assert_eq!(body.size_hint().exact(), Some(12));
        assert_eq!(collect(body).await, b"0123456789ab");
    }

    #[tokio::test]
    async fn body_continues_with_the_stream_after_the_spool() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, b"0123456789", &config);
        append(&mut spooled, &mut output, b"ab", &config);
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(Bytes::from_static(b"cd")).await.unwrap();
        drop(tx);

        let spool = spooled.take(&mut output, &config);
        let body = body(spool, Bytes::from(output), Some(rx));
        assert_eq!(body.size_hint().exact(), None);
        assert_eq!(collect(body).await, b"0123456789abcd");
    }

    #[tokio::test]
    async fn the_budget_comes_back_once_the_file_is_read_while_the_stream_goes_on() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, &[1u8; 100], &config);
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(Bytes::from_static(b"cd")).await.unwrap();

        let spool = spooled.take(&mut output, &config);
        let mut body = body(spool, Bytes::new(), Some(rx));
        let mut read = 0;
        while read < 102 {
            let frame = body.frame().await.unwrap().unwrap();
            read += frame.data_ref().unwrap().len();
        }
        assert_eq!(settled_in_use(config.budget).await, 0);
        assert!(!tx.is_closed());
    }

    #[tokio::test]
    async fn dropping_an_unread_body_releases_the_budget() {
        let (config, _dir) = test_config(8, 1024);
        let mut spooled = Spooled::default();
        let mut output = Vec::new();
        append(&mut spooled, &mut output, &[1u8; 100], &config);

        let spool = spooled.take(&mut output, &config);
        let body = body(spool, Bytes::new(), None);
        assert_eq!(config.budget.in_use(), 100);
        drop(body);
        assert_eq!(settled_in_use(config.budget).await, 0);
    }

    #[test]
    fn an_empty_tmpdir_counts_as_unset() {
        assert_eq!(spool_dir(None), PathBuf::from("/tmp"));
        assert_eq!(spool_dir(Some(OsString::new())), PathBuf::from("/tmp"));
        assert_eq!(
            spool_dir(Some(OsString::from("/var/spool"))),
            PathBuf::from("/var/spool")
        );
    }

    #[tokio::test]
    async fn body_without_a_spool_is_the_in_memory_output() {
        let body = body(None, Bytes::from_static(b"hello"), None);
        assert_eq!(body.size_hint().exact(), Some(5));
        assert_eq!(collect(body).await, b"hello");
    }
}
