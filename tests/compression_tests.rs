//! End-to-end content-coding negotiation: what the client asks for in
//! `Accept-Encoding` against what comes back on the wire.
//!
//! The unit tests cover the codecs and the header grammar; this suite covers
//! the wiring between them — that the negotiated coding is the one actually
//! applied, at its own configured level, and that a client asking for nothing
//! still gets a body it can read.

mod common;

use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;

use oxphp::events::EventDispatcher;
use oxphp::server::compression::Levels;

/// Long enough to clear the 256-byte compression floor by a wide margin, and
/// repetitive enough that every level shrinks it.
fn css_body() -> String {
    "body { color: rebeccapurple; margin: 0 }\n".repeat(40)
}

async fn start_server(document_root: &std::path::Path, compression: Levels) -> SocketAddr {
    let metrics = Arc::new(oxphp::metrics::Metrics::new());
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    let (addr, _server) = common::start_test_server_with_compression(
        document_root,
        &oxphp::config::H2Config::default(),
        None,
        metrics,
        dispatcher,
        compression,
    )
    .await;
    addr
}

/// Fetch `/app.css` with an explicit `Accept-Encoding`, or none at all.
/// Returns the coding the server chose and the raw (still encoded) body.
async fn fetch(addr: SocketAddr, accept_encoding: Option<&str>) -> (Option<String>, Vec<u8>) {
    let mut request = reqwest::Client::new().get(format!("http://{addr}/app.css"));
    if let Some(value) = accept_encoding {
        request = request.header("accept-encoding", value);
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), 200);
    let coding = response
        .headers()
        .get("content-encoding")
        .map(|v| v.to_str().unwrap().to_string());
    let vary_names_encoding = response
        .headers()
        .get_all("vary")
        .iter()
        .flat_map(|v| v.to_str().unwrap().split(','))
        .any(|member| member.trim().eq_ignore_ascii_case("accept-encoding"));
    assert!(
        vary_names_encoding,
        "a response whose body depends on Accept-Encoding must say so"
    );
    let body = response.bytes().await.unwrap().to_vec();
    assert_eq!(coding.is_some(), body != css_body().as_bytes());
    (coding, body)
}

fn unzstd(body: &[u8]) -> Vec<u8> {
    zstd::decode_all(body).unwrap()
}

fn gunzip(body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(body)
        .read_to_end(&mut decoded)
        .unwrap();
    decoded
}

#[tokio::test]
async fn a_gzip_only_client_gets_a_readable_gzip_body() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        },
    )
    .await;

    let (coding, body) = fetch(addr, Some("gzip, deflate")).await;

    assert_eq!(coding.as_deref(), Some("gzip"));
    assert_eq!(gunzip(&body), css_body().as_bytes());
}

#[tokio::test]
async fn a_client_that_accepts_everything_gets_zstd() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        },
    )
    .await;

    // The header every current browser sends: no weights, so the choice is
    // ours, and for bytes compressed to answer one request it is zstd.
    let (coding, body) = fetch(addr, Some("gzip, deflate, br, zstd")).await;

    assert_eq!(coding.as_deref(), Some("zstd"));
    assert_eq!(unzstd(&body), css_body().as_bytes());
}

#[tokio::test]
async fn a_cached_static_file_hands_over_to_its_brotli_copy() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        },
    )
    .await;

    // Same client, same file, and the coding changes under it: the first hits
    // are compressed to answer that one request, where zstd is cheapest, while
    // the stored copy is built at brotli's top quality, where it is smallest.
    // Both are correct representations and both carry Vary, so a cache that
    // kept the first one is not wrong — it is only holding the larger body.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (coding, body) = fetch(addr, Some("gzip, deflate, br, zstd")).await;
        if coding.as_deref() == Some("br") {
            assert!(body.len() < css_body().len());
            break;
        }
        assert_eq!(coding.as_deref(), Some("zstd"));
        assert!(
            std::time::Instant::now() < deadline,
            "the brotli copy never took over"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_clients_weights_outrank_server_preference() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        },
    )
    .await;

    let (coding, body) = fetch(addr, Some("br;q=0.2, gzip;q=0.9")).await;

    assert_eq!(coding.as_deref(), Some("gzip"));
    assert_eq!(gunzip(&body), css_body().as_bytes());
}

#[tokio::test]
async fn a_refused_coding_is_not_sent() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        },
    )
    .await;

    // Brotli refused outright, gzip never named: identity is all that is left.
    let (coding, body) = fetch(addr, Some("br;q=0")).await;

    assert_eq!(coding, None);
    assert_eq!(body, css_body().as_bytes());
}

#[tokio::test]
async fn no_accept_encoding_means_no_encoding() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        },
    )
    .await;

    let (coding, body) = fetch(addr, None).await;

    assert_eq!(coding, None);
    assert_eq!(body, css_body().as_bytes());
}

#[tokio::test]
async fn gzip_level_zero_leaves_a_gzip_only_client_unencoded() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("app.css"), css_body()).unwrap();
    let addr = start_server(
        dir.path(),
        Levels {
            brotli: 5,
            gzip: 0,
            zstd: 0,
        },
    )
    .await;

    let (gzip_client, body) = fetch(addr, Some("gzip")).await;
    assert_eq!(gzip_client, None);
    assert_eq!(body, css_body().as_bytes());

    // Clients that reach a coding still on are unaffected by that switch.
    let (brotli_client, _) = fetch(addr, Some("gzip, br")).await;
    assert_eq!(brotli_client.as_deref(), Some("br"));
}
