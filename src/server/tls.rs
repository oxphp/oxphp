use std::path::Path;
use std::sync::Arc;

use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// Load TLS configuration from PEM cert and key files.
/// Returns a `TlsAcceptor` configured with ALPN for h2 + http/1.1.
pub fn load_tls_config(
    cert_path: &Path,
    key_path: &Path,
) -> Result<TlsAcceptor, crate::types::BoxError> {
    let cert_data = std::fs::read(cert_path)?;
    let key_data = std::fs::read(key_path)?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_data.as_slice()).collect::<Result<Vec<_>, _>>()?;

    let key = rustls_pemfile::private_key(&mut key_data.as_slice())?
        .ok_or("no private key found in PEM file")?;

    let provider = rustls::crypto::ring::default_provider();

    let mut config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(config)))
}
