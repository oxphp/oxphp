use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use http::HeaderValue;

use crate::events::ResponseBuilding;
use crate::events::{EventHandler, Priority, Propagation};

/// Cached `Date` header value with 1-second resolution.
/// Avoids `SystemTime::now()` + `httpdate::fmt_http_date()` + `HeaderValue::from_str()`
/// on every response — replaces 2 allocations + syscall with a single atomic load
/// plus a cheap `Mutex` take on the value clone.
struct CachedDate {
    /// Unix epoch second when the cached value was generated.
    epoch_sec: AtomicU64,
    /// Pre-built `HeaderValue`. Guarded by `Mutex` rather than `RwLock` because
    /// `std::sync::RwLock::read()` on Linux (pthread_rwlock_t) is measurably
    /// slower uncontended than a futex-backed `Mutex::lock()`, and writes are
    /// rare (at most once per second, gated by an `AtomicU64` CAS).
    value: Mutex<HeaderValue>,
}

static CACHED_DATE: OnceLock<CachedDate> = OnceLock::new();

/// Get a `Date` header value, reusing the cached one if still within the same second.
#[inline]
fn get_date_header() -> HeaderValue {
    let now = SystemTime::now();
    let epoch_sec = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let cached = CACHED_DATE.get_or_init(|| {
        let formatted = httpdate::fmt_http_date(now);
        CachedDate {
            epoch_sec: AtomicU64::new(epoch_sec),
            value: Mutex::new(HeaderValue::from_str(&formatted).unwrap()),
        }
    });

    let prev = cached.epoch_sec.load(Ordering::Relaxed);
    if prev == epoch_sec {
        // Same second — return cached value.
        return cached
            .value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
    }

    // Second changed — try to be the updater (CAS to avoid thundering herd)
    if cached
        .epoch_sec
        .compare_exchange(prev, epoch_sec, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        let formatted = httpdate::fmt_http_date(now);
        let hv = HeaderValue::from_str(&formatted).unwrap();
        *cached.value.lock().unwrap_or_else(|e| e.into_inner()) = hv.clone();
        return hv;
    }

    // Another thread is updating — read whatever is there (at most 1s stale)
    cached
        .value
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Adds `Server`, `Date`, and `X-Request-ID` headers to every response.
pub struct ServerHeaderHandler;

impl EventHandler<ResponseBuilding> for ServerHeaderHandler {
    #[inline]
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        let headers = event.response.headers_mut();

        headers.insert(http::header::SERVER, HeaderValue::from_static("OxPHP"));
        headers.insert(http::header::DATE, get_date_header());

        if let Ok(hv) = HeaderValue::from_str(&event.request_id) {
            headers.insert("x-request-id", hv);
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use crate::types::full_body;
    use bytes::Bytes;
    use http::Response;

    #[test]
    fn test_adds_server_header() {
        let handler = ServerHeaderHandler;
        let mut event = ResponseBuilding {
            request_id: "abc123".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        assert!(event.response.headers().contains_key("server"));
        assert_eq!(
            event
                .response
                .headers()
                .get("server")
                .unwrap()
                .to_str()
                .unwrap(),
            "OxPHP"
        );
    }

    #[test]
    fn test_adds_request_id_header() {
        let handler = ServerHeaderHandler;
        let mut event = ResponseBuilding {
            request_id: "deadbeef12345678".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        assert_eq!(
            event
                .response
                .headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "deadbeef12345678"
        );
    }

    #[test]
    fn test_adds_date_header() {
        let handler = ServerHeaderHandler;
        let mut event = ResponseBuilding {
            request_id: "abc123".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        let date = event
            .response
            .headers()
            .get("date")
            .expect("Date header must be present")
            .to_str()
            .unwrap();
        assert!(date.ends_with(" GMT"), "Date must end with GMT: {date}");
        // Validate it parses back as valid HTTP-date
        httpdate::parse_http_date(date).expect("Date must be valid HTTP-date");
    }

    #[test]
    fn test_priority() {
        assert_eq!(ServerHeaderHandler.priority(), 100);
    }
}
