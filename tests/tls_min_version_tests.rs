//! Integration tests for the TLS protocol-version floor (`TLS_MIN_VERSION`).
//!
//! These drive real rustls handshakes against the `TlsAcceptor` produced by
//! `load_tls_config`, using a throwaway self-signed certificate generated at
//! test runtime (rcgen) — no private key lives in the repository. The client
//! restricts itself to a single protocol version per test, so each case pins
//! down exactly what the negotiated version (or the rejection) proves:
//!
//! * floor `1.2` (the default) completes both a TLS 1.2-only and a
//!   TLS 1.3-only handshake — the historical behavior;
//! * floor `1.3` rejects a TLS 1.2-only client at the handshake and still
//!   completes a TLS 1.3-only one.
//!
//! Certificate verification is disabled on the client (`AcceptAnyCert`):
//! these tests are about version negotiation, not trust chains.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, ProtocolVersion, SupportedProtocolVersion};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use oxphp::config::TlsMinVersion;
use oxphp::server::tls::load_tls_config;

/// Write a freshly generated self-signed localhost cert + key into `dir`
/// and return their paths.
fn generate_cert(dir: &std::path::Path) -> (PathBuf, PathBuf) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("generate self-signed cert");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, certified.cert.pem()).unwrap();
    std::fs::write(&key_path, certified.key_pair.serialize_pem()).unwrap();
    (cert_path, key_path)
}

/// Client-side verifier that accepts any server certificate. The test cert
/// is self-signed; trust evaluation is irrelevant to version negotiation.
#[derive(Debug)]
struct AcceptAnyCert(rustls::crypto::CryptoProvider);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn client_config(versions: &[&'static SupportedProtocolVersion]) -> ClientConfig {
    let provider = rustls::crypto::ring::default_provider();
    let verifier = Arc::new(AcceptAnyCert(provider.clone()));
    ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(versions)
        .expect("client protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

/// Run one handshake: server floor `min_version` vs a client limited to
/// `client_versions`. Returns the negotiated version on success.
async fn handshake(
    min_version: TlsMinVersion,
    client_versions: &[&'static SupportedProtocolVersion],
) -> Result<ProtocolVersion, std::io::Error> {
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = generate_cert(dir.path());
    let acceptor =
        load_tls_config(&cert_path, &key_path, min_version).expect("load acceptor from temp cert");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // The client side asserts the outcome; a rejected handshake is an
        // expected error here.
        let _ = acceptor.accept(stream).await;
    });

    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config(client_versions)));
    let tcp = TcpStream::connect(addr).await?;
    let server_name = ServerName::try_from("localhost").unwrap();

    let result = timeout(Duration::from_secs(5), connector.connect(server_name, tcp))
        .await
        .expect("handshake should not hang");

    timeout(Duration::from_secs(5), server)
        .await
        .expect("server task should finish")
        .unwrap();

    result.map(|tls| {
        tls.get_ref()
            .1
            .protocol_version()
            .expect("version set after handshake")
    })
}

#[tokio::test]
async fn floor_12_accepts_tls12_client() {
    let version = handshake(TlsMinVersion::V12, &[&rustls::version::TLS12])
        .await
        .expect("TLS 1.2 handshake should succeed with the default floor");
    assert_eq!(version, ProtocolVersion::TLSv1_2);
}

#[tokio::test]
async fn floor_12_accepts_tls13_client() {
    let version = handshake(TlsMinVersion::V12, &[&rustls::version::TLS13])
        .await
        .expect("TLS 1.3 handshake should succeed with the default floor");
    assert_eq!(version, ProtocolVersion::TLSv1_3);
}

#[tokio::test]
async fn floor_13_rejects_tls12_client() {
    let err = handshake(TlsMinVersion::V13, &[&rustls::version::TLS12])
        .await
        .expect_err("TLS 1.2 client must be rejected when the floor is 1.3");
    // Match the typed rustls error, not its Display text — the server answers
    // the 1.2-only ClientHello with a fatal protocol_version alert.
    let alert = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    assert!(
        matches!(
            alert,
            Some(rustls::Error::AlertReceived(
                rustls::AlertDescription::ProtocolVersion
            ))
        ),
        "expected a protocol_version alert, got: {err:?}"
    );
}

#[tokio::test]
async fn floor_13_accepts_tls13_client() {
    let version = handshake(TlsMinVersion::V13, &[&rustls::version::TLS13])
        .await
        .expect("TLS 1.3 handshake should succeed with a 1.3 floor");
    assert_eq!(version, ProtocolVersion::TLSv1_3);
}
