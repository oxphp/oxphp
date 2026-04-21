use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::config::TrustedProxyConfig;
use crate::events::RequestReceived;
use crate::events::{EventHandler, Priority, Propagation};

pub struct TrustedProxyHandler {
    config: Arc<TrustedProxyConfig>,
}

impl TrustedProxyHandler {
    pub fn new(config: Arc<TrustedProxyConfig>) -> Self {
        Self { config }
    }
}

impl EventHandler<RequestReceived> for TrustedProxyHandler {
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        if !self.config.is_trusted(event.remote_addr.ip()) {
            return Propagation::Continue;
        }

        event
            .metadata
            .push(("peer_addr".into(), event.remote_addr.to_string()));

        // Collect all Forwarded header values (RFC 7230 §3.2.2: may span multiple lines)
        let forwarded_combined: Option<String> = {
            let values: Vec<&str> = event
                .parts
                .headers
                .get_all("forwarded")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();
            if values.is_empty() {
                None
            } else {
                Some(values.join(", "))
            }
        };

        if let Some(ref fwd_value) = forwarded_combined {
            let entries = parse_forwarded(fwd_value);

            let for_ips: Vec<IpAddr> = entries.iter().filter_map(|e| e.forwarded_for).collect();
            if let Some(client_ip) = self.extract_client_ip(&for_ips) {
                event.remote_addr = SocketAddr::new(client_ip, 0);
            }

            if let Some(proto) = entries.first().and_then(|e| e.proto) {
                event
                    .metadata
                    .push(("forwarded_proto".into(), proto.to_string()));
            }

            if let Some(host) = entries.first().and_then(|e| e.host) {
                event
                    .metadata
                    .push(("forwarded_host".into(), host.to_string()));
            }
        } else {
            // Collect all X-Forwarded-For values (may span multiple header lines)
            let xff_values: Vec<&str> = event
                .parts
                .headers
                .get_all("x-forwarded-for")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();
            if !xff_values.is_empty() {
                let ips: Vec<IpAddr> = xff_values
                    .join(", ")
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if let Some(client_ip) = self.extract_client_ip(&ips) {
                    event.remote_addr = SocketAddr::new(client_ip, 0);
                }
            }

            if let Some(proto) = event
                .parts
                .headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
            {
                event
                    .metadata
                    .push(("forwarded_proto".into(), proto.trim().to_string()));
            }

            if let Some(host) = event
                .parts
                .headers
                .get("x-forwarded-host")
                .and_then(|v| v.to_str().ok())
            {
                event
                    .metadata
                    .push(("forwarded_host".into(), host.trim().to_string()));
            }
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -80
    }
}

impl TrustedProxyHandler {
    fn extract_client_ip(&self, chain: &[IpAddr]) -> Option<IpAddr> {
        if chain.is_empty() {
            return None;
        }
        for ip in chain.iter().rev() {
            if !self.config.is_trusted(*ip) {
                return Some(*ip);
            }
        }
        Some(chain[0])
    }
}

#[derive(Debug, Default)]
struct ForwardedEntry<'a> {
    forwarded_for: Option<IpAddr>,
    proto: Option<&'a str>,
    host: Option<&'a str>,
}

fn parse_forwarded(value: &str) -> Vec<ForwardedEntry<'_>> {
    value
        .split(',')
        .map(|element| {
            let mut entry = ForwardedEntry::default();
            for pair in element.split(';') {
                let pair = pair.trim();
                if let Some((key, val)) = pair.split_once('=') {
                    let key = key.trim().to_ascii_lowercase();
                    let val = val.trim();
                    match key.as_str() {
                        "for" => {
                            entry.forwarded_for = parse_forwarded_for(val);
                        }
                        "proto" => {
                            entry.proto = Some(val);
                        }
                        "host" => {
                            entry.host = Some(val);
                        }
                        _ => {}
                    }
                }
            }
            entry
        })
        .collect()
}

fn parse_forwarded_for(val: &str) -> Option<IpAddr> {
    let val = val.trim_matches('"');
    let val = val.trim_start_matches('[').trim_end_matches(']');
    // Try direct parse first (handles plain IPv4, IPv6, and IPv4-mapped IPv6)
    if let Ok(ip) = val.parse::<IpAddr>() {
        return Some(ip);
    }
    // Fallback: strip port suffix (e.g. "203.0.113.50:1234")
    val.rsplit_once(':').and_then(|(ip, _)| ip.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;

    fn make_config(spec: &str) -> Arc<TrustedProxyConfig> {
        Arc::new(TrustedProxyConfig::from_spec(spec))
    }

    fn make_event_with_headers(
        remote_addr: SocketAddr,
        headers: Vec<(&str, &str)>,
    ) -> RequestReceived {
        let mut builder = http::Request::builder()
            .method(http::Method::GET)
            .uri("/test");
        for (k, v) in &headers {
            builder = builder.header(*k, *v);
        }
        let (parts, _) = builder.body(()).unwrap().into_parts();
        RequestReceived {
            parts,
            remote_addr,
            request_id: "test123".to_string(),
            early_response: None,
            metadata: Vec::new(),
            profiling_mode: None,
            profiling_run_id: None,
        }
    }

    fn trusted_addr() -> SocketAddr {
        SocketAddr::new(std::net::Ipv4Addr::new(10, 0, 0, 1).into(), 54321)
    }

    fn untrusted_addr() -> SocketAddr {
        SocketAddr::new(std::net::Ipv4Addr::new(203, 0, 113, 99).into(), 54321)
    }

    // ── parse_forwarded_for ──────────────────────────────────────────────────

    #[test]
    fn test_parse_forwarded_for_ipv4() {
        let ip = parse_forwarded_for("203.0.113.50");
        assert_eq!(ip, Some("203.0.113.50".parse().unwrap()));
    }

    #[test]
    fn test_parse_forwarded_for_ipv4_quoted() {
        let ip = parse_forwarded_for("\"203.0.113.50\"");
        assert_eq!(ip, Some("203.0.113.50".parse().unwrap()));
    }

    #[test]
    fn test_parse_forwarded_for_ipv4_with_port() {
        let ip = parse_forwarded_for("203.0.113.50:1234");
        assert_eq!(ip, Some("203.0.113.50".parse().unwrap()));
    }

    #[test]
    fn test_parse_forwarded_for_ipv6_bracketed() {
        let ip = parse_forwarded_for("[2001:db8::1]");
        assert_eq!(ip, Some("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn test_parse_forwarded_for_ipv6_bare() {
        let ip = parse_forwarded_for("2001:db8::1");
        assert_eq!(ip, Some("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn test_parse_forwarded_for_invalid() {
        let ip = parse_forwarded_for("not-an-ip");
        assert_eq!(ip, None);
    }

    // ── parse_forwarded ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_forwarded_single_entry() {
        let entries = parse_forwarded("for=203.0.113.50;proto=https;host=example.com");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].forwarded_for,
            Some("203.0.113.50".parse().unwrap())
        );
        assert_eq!(entries[0].proto, Some("https"));
        assert_eq!(entries[0].host, Some("example.com"));
    }

    #[test]
    fn test_parse_forwarded_chain() {
        let entries = parse_forwarded("for=203.0.113.50,for=10.0.0.1");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].forwarded_for,
            Some("203.0.113.50".parse().unwrap())
        );
        assert_eq!(entries[1].forwarded_for, Some("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_parse_forwarded_ipv6() {
        let entries = parse_forwarded("for=\"[2001:db8::1]\";proto=https");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].forwarded_for,
            Some("2001:db8::1".parse().unwrap())
        );
        assert_eq!(entries[0].proto, Some("https"));
    }

    #[test]
    fn test_parse_forwarded_case_insensitive_keys() {
        let entries = parse_forwarded("FOR=203.0.113.50;PROTO=https;HOST=example.com");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].forwarded_for,
            Some("203.0.113.50".parse().unwrap())
        );
        assert_eq!(entries[0].proto, Some("https"));
        assert_eq!(entries[0].host, Some("example.com"));
    }

    // ── extract_client_ip ────────────────────────────────────────────────────

    #[test]
    fn test_extract_client_ip_single_untrusted() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let chain: Vec<IpAddr> = vec!["203.0.113.50".parse().unwrap()];
        assert_eq!(
            handler.extract_client_ip(&chain),
            Some("203.0.113.50".parse().unwrap())
        );
    }

    #[test]
    fn test_extract_client_ip_rightmost_non_trusted() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        // Chain: [client, proxy1(trusted), proxy2(trusted)]
        // rightmost non-trusted = client
        let chain: Vec<IpAddr> = vec![
            "203.0.113.50".parse().unwrap(), // client
            "10.0.0.1".parse().unwrap(),     // trusted proxy
            "10.0.0.2".parse().unwrap(),     // trusted proxy
        ];
        assert_eq!(
            handler.extract_client_ip(&chain),
            Some("203.0.113.50".parse().unwrap())
        );
    }

    #[test]
    fn test_extract_client_ip_multi_hop() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        // Chain: [client, untrusted-intermediate, trusted-proxy]
        // rightmost non-trusted = untrusted-intermediate (not the very first)
        let chain: Vec<IpAddr> = vec![
            "203.0.113.50".parse().unwrap(), // client
            "198.51.100.1".parse().unwrap(), // untrusted intermediate
            "10.0.0.1".parse().unwrap(),     // trusted proxy
        ];
        assert_eq!(
            handler.extract_client_ip(&chain),
            Some("198.51.100.1".parse().unwrap())
        );
    }

    #[test]
    fn test_extract_client_ip_all_trusted_returns_leftmost() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let chain: Vec<IpAddr> = vec![
            "10.0.0.5".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
            "10.0.0.2".parse().unwrap(),
        ];
        assert_eq!(
            handler.extract_client_ip(&chain),
            Some("10.0.0.5".parse().unwrap())
        );
    }

    #[test]
    fn test_extract_client_ip_empty_chain() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        assert_eq!(handler.extract_client_ip(&[]), None);
    }

    #[test]
    fn test_extract_client_ip_spoofed_prefix() {
        // Attacker appends a spoofed trusted IP at the start of XFF chain.
        // XFF: [spoofed(10.0.0.1), attacker(evil.ip), trusted-proxy(10.0.0.2)]
        // rightmost-non-trusted = attacker IP, not the spoofed one
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let chain: Vec<IpAddr> = vec![
            "10.0.0.1".parse().unwrap(),     // spoofed by attacker
            "203.0.113.99".parse().unwrap(), // attacker's real IP
            "10.0.0.2".parse().unwrap(),     // trusted proxy
        ];
        assert_eq!(
            handler.extract_client_ip(&chain),
            Some("203.0.113.99".parse().unwrap())
        );
    }

    // ── Handler integration ──────────────────────────────────────────────────

    #[test]
    fn test_handler_skips_untrusted_peer() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event =
            make_event_with_headers(untrusted_addr(), vec![("x-forwarded-for", "1.2.3.4")]);
        let original_addr = event.remote_addr;
        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
        // remote_addr must be unchanged
        assert_eq!(event.remote_addr, original_addr);
        // metadata must be empty (no peer_addr saved)
        assert!(event.metadata.is_empty());
    }

    #[test]
    fn test_handler_xff_basic() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event =
            make_event_with_headers(trusted_addr(), vec![("x-forwarded-for", "203.0.113.50")]);
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        assert_eq!(event.remote_addr.port(), 0);
    }

    #[test]
    fn test_handler_xff_with_proto_and_host() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "203.0.113.50"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "example.com"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        let proto = event
            .metadata
            .iter()
            .find(|(k, _)| k == "forwarded_proto")
            .map(|(_, v)| v.as_str());
        assert_eq!(proto, Some("https"));
        let host = event
            .metadata
            .iter()
            .find(|(k, _)| k == "forwarded_host")
            .map(|(_, v)| v.as_str());
        assert_eq!(host, Some("example.com"));
    }

    #[test]
    fn test_handler_forwarded_rfc7239() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![("forwarded", "for=203.0.113.50;proto=https;host=example.com")],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        let proto = event
            .metadata
            .iter()
            .find(|(k, _)| k == "forwarded_proto")
            .map(|(_, v)| v.as_str());
        assert_eq!(proto, Some("https"));
        let host = event
            .metadata
            .iter()
            .find(|(k, _)| k == "forwarded_host")
            .map(|(_, v)| v.as_str());
        assert_eq!(host, Some("example.com"));
    }

    #[test]
    fn test_handler_forwarded_takes_priority_over_xff() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("forwarded", "for=203.0.113.50;proto=https"),
                ("x-forwarded-for", "1.2.3.4"),
                ("x-forwarded-proto", "http"),
            ],
        );
        handler.handle(&mut event);
        // Must use Forwarded, not XFF
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        let proto = event
            .metadata
            .iter()
            .find(|(k, _)| k == "forwarded_proto")
            .map(|(_, v)| v.as_str());
        assert_eq!(proto, Some("https"));
    }

    #[test]
    fn test_handler_no_headers_preserves_peer_addr_in_metadata() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(trusted_addr(), vec![]);
        let original_addr = event.remote_addr;
        handler.handle(&mut event);
        // remote_addr unchanged (no forwarding headers)
        assert_eq!(event.remote_addr, original_addr);
        // peer_addr must be saved in metadata
        let peer = event
            .metadata
            .iter()
            .find(|(k, _)| k == "peer_addr")
            .map(|(_, v)| v.as_str());
        assert_eq!(peer, Some(original_addr.to_string().as_str()));
    }

    #[test]
    fn test_handler_priority_is_minus_80() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        assert_eq!(handler.priority(), -80);
    }
}
