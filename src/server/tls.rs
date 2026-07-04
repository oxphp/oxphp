use std::path::Path;
use std::sync::Arc;

use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

use crate::config::TlsMinVersion;

/// The rustls protocol-version set implementing a `TLS_MIN_VERSION` floor.
fn protocol_versions(
    min_version: TlsMinVersion,
) -> &'static [&'static rustls::SupportedProtocolVersion] {
    static TLS12_AND_UP: &[&rustls::SupportedProtocolVersion] =
        &[&rustls::version::TLS13, &rustls::version::TLS12];
    static TLS13_ONLY: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];
    match min_version {
        TlsMinVersion::V12 => TLS12_AND_UP,
        TlsMinVersion::V13 => TLS13_ONLY,
    }
}

/// Load TLS configuration from PEM cert and key files.
/// Returns a `TlsAcceptor` configured with ALPN for h2 + http/1.1 that
/// accepts protocol versions down to `min_version`.
pub fn load_tls_config(
    cert_path: &Path,
    key_path: &Path,
    min_version: TlsMinVersion,
) -> Result<TlsAcceptor, crate::types::BoxError> {
    // Every failure names the env var and the path — a typo'd TLS_CERT must
    // not fail startup with a bare "No such file or directory (os error 2)".
    let cert_data = std::fs::read(cert_path)
        .map_err(|e| format!("TLS_CERT: cannot read {}: {e}", cert_path.display()))?;
    let key_data = std::fs::read(key_path)
        .map_err(|e| format!("TLS_KEY: cannot read {}: {e}", key_path.display()))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_data.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("TLS_CERT: invalid PEM in {}: {e}", cert_path.display()))?;

    let key = rustls_pemfile::private_key(&mut key_data.as_slice())
        .map_err(|e| format!("TLS_KEY: invalid PEM in {}: {e}", key_path.display()))?
        .ok_or_else(|| format!("TLS_KEY: no private key found in {}", key_path.display()))?;

    let provider = rustls::crypto::ring::default_provider();

    let mut config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(protocol_versions(min_version))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS_CERT/TLS_KEY: invalid certificate or key: {e}"))?;

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_error_names_the_variable_and_path() {
        let err = match load_tls_config(
            Path::new("/nonexistent/cert.pem"),
            Path::new("/nonexistent/key.pem"),
            TlsMinVersion::V12,
        ) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for a missing cert path"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("TLS_CERT") && msg.contains("/nonexistent/cert.pem"),
            "msg: {msg}"
        );
    }

    #[test]
    fn protocol_versions_match_floor() {
        let v12: Vec<_> = protocol_versions(TlsMinVersion::V12)
            .iter()
            .map(|v| v.version)
            .collect();
        assert_eq!(
            v12,
            [
                rustls::ProtocolVersion::TLSv1_3,
                rustls::ProtocolVersion::TLSv1_2
            ]
        );

        let v13: Vec<_> = protocol_versions(TlsMinVersion::V13)
            .iter()
            .map(|v| v.version)
            .collect();
        assert_eq!(v13, [rustls::ProtocolVersion::TLSv1_3]);
    }
}
