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
        let peer = event.remote_addr.ip();
        if !self.config.is_trusted(peer) {
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

        let chain: Chain;

        if let Some(ref fwd_value) = forwarded_combined {
            let entries = parse_forwarded(fwd_value);

            chain = entries
                .iter()
                .map(|e| e.forwarded_for.map(|ip| (ip, e.forwarded_for_port)))
                .collect();

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
            // X-Forwarded-For carries no source port, so every node pairs
            // with `None` and the rewritten REMOTE_PORT stays 0.
            chain = if xff_values.is_empty() {
                Chain::default()
            } else {
                xff_values
                    .join(", ")
                    .split(',')
                    .map(|s| s.trim().parse().ok().map(|ip| (ip, None)))
                    .collect()
            };

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

            // X-Forwarded-Port: the public port the proxy listens on. A single
            // numeric value (1..=65535); `u16` parsing bounds the upper end,
            // the `> 0` filter the lower. Drives SERVER_PORT over the host suffix.
            if let Some(port) = event
                .parts
                .headers
                .get("x-forwarded-port")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u16>().ok())
                .filter(|p| *p > 0)
            {
                event
                    .metadata
                    .push(("forwarded_port".into(), port.to_string()));
            }
        }

        if let Some((client_ip, client_port)) =
            self.resolve_client(peer, &event.parts.headers, &chain)
        {
            event.remote_addr = SocketAddr::new(client_ip, client_port.unwrap_or(0));
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -80
    }
}

impl TrustedProxyHandler {
    /// The client address: `CF-Connecting-IP` when the request came through a
    /// Cloudflare edge and carries a usable one, otherwise rightmost-non-trusted
    /// over the parsed hops.
    fn resolve_client(
        &self,
        peer: IpAddr,
        headers: &http::HeaderMap,
        chain: &Chain,
    ) -> Option<(IpAddr, Option<u16>)> {
        if self.came_through_cloudflare(peer, chain.unbroken_tail()) {
            if let Some(ip) = cf_connecting_ip(headers) {
                return Some((ip, None));
            }
        }
        self.extract_client_ip(&chain.hops)
    }

    /// Whether the first hop outside the operator's own proxies, walking left
    /// from the peer through `hops`, is a Cloudflare edge. That hop is the peer
    /// itself or an address one of those proxies recorded, so a client cannot
    /// place a Cloudflare address there — provided those proxies maintain the
    /// header the chain was read from, which for `Forwarded` includes not
    /// passing a client's own through. A walk that runs out of hops finds none.
    fn came_through_cloudflare(&self, peer: IpAddr, hops: &[(IpAddr, Option<u16>)]) -> bool {
        std::iter::once(peer)
            .chain(hops.iter().rev().map(|&(ip, _)| ip))
            .find(|&ip| !self.config.is_own_proxy(ip))
            .is_some_and(|ip| self.config.is_cloudflare(ip))
    }

    /// Rightmost-non-trusted selection over a forwarding chain. The trust check
    /// uses only the IP (`.0`); the optional source port rides along so the
    /// caller can recover `REMOTE_PORT` from an RFC 7239 `for=ip:port` node.
    fn extract_client_ip(&self, chain: &[(IpAddr, Option<u16>)]) -> Option<(IpAddr, Option<u16>)> {
        if chain.is_empty() {
            return None;
        }
        for entry in chain.iter().rev() {
            if !self.config.is_trusted(entry.0) {
                return Some(*entry);
            }
        }
        Some(chain[0])
    }
}

/// A forwarding chain as read from `Forwarded` or `X-Forwarded-For`.
#[derive(Debug, Default)]
struct Chain {
    /// The hops whose address parsed, leftmost first.
    hops: Vec<(IpAddr, Option<u16>)>,
    /// How many of `hops` come after the rightmost hop whose address is
    /// missing or did not parse; all of them when there is no such hop.
    unbroken: usize,
}

impl Chain {
    /// The hops right of the last one that is missing or did not parse —
    /// as far as a walk from the peer can vouch for.
    fn unbroken_tail(&self) -> &[(IpAddr, Option<u16>)] {
        &self.hops[self.hops.len() - self.unbroken..]
    }
}

impl FromIterator<Option<(IpAddr, Option<u16>)>> for Chain {
    fn from_iter<I: IntoIterator<Item = Option<(IpAddr, Option<u16>)>>>(iter: I) -> Self {
        let mut chain = Chain::default();
        for hop in iter {
            match hop {
                Some(hop) => {
                    chain.hops.push(hop);
                    chain.unbroken += 1;
                }
                None => chain.unbroken = 0,
            }
        }
        chain
    }
}

/// The `CF-Connecting-IP` address, if the request carries exactly one such
/// header and it parses as an IP.
fn cf_connecting_ip(headers: &http::HeaderMap) -> Option<IpAddr> {
    let mut values = headers.get_all("cf-connecting-ip").iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()?.trim().parse().ok()
}

#[derive(Debug, Default)]
struct ForwardedEntry<'a> {
    forwarded_for: Option<IpAddr>,
    /// Source port from `for=ip:port`, if the node carried one.
    forwarded_for_port: Option<u16>,
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
                            if let Some((ip, port)) = parse_forwarded_for(val) {
                                entry.forwarded_for = Some(ip);
                                entry.forwarded_for_port = port;
                            }
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

/// Parse a single RFC 7239 `for=` node value into its IP and optional source
/// port. Handles `[ipv6]:port`, `[ipv6]`, bare ipv6, `ipv4:port` and bare ipv4.
/// A port of `0` (or an unparseable port) is normalized to `None`.
fn parse_forwarded_for(val: &str) -> Option<(IpAddr, Option<u16>)> {
    let val = val.trim_matches('"').trim();
    // Bracketed IPv6 literal, optionally followed by `:port`.
    if let Some(rest) = val.strip_prefix('[') {
        let (ip_part, after) = rest.split_once(']')?;
        let ip = ip_part.parse::<IpAddr>().ok()?;
        let port = after
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p > 0);
        return Some((ip, port));
    }
    // Bare IP (covers unbracketed IPv6 and plain IPv4 without a port).
    if let Ok(ip) = val.parse::<IpAddr>() {
        return Some((ip, None));
    }
    // `ipv4:port`.
    let (ip_part, port_part) = val.rsplit_once(':')?;
    let ip = ip_part.parse::<IpAddr>().ok()?;
    let port = port_part.parse::<u16>().ok().filter(|p| *p > 0);
    Some((ip, port))
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
        let parsed = parse_forwarded_for("203.0.113.50");
        assert_eq!(parsed, Some(("203.0.113.50".parse().unwrap(), None)));
    }

    #[test]
    fn test_parse_forwarded_for_ipv4_quoted() {
        let parsed = parse_forwarded_for("\"203.0.113.50\"");
        assert_eq!(parsed, Some(("203.0.113.50".parse().unwrap(), None)));
    }

    #[test]
    fn test_parse_forwarded_for_ipv4_with_port() {
        let parsed = parse_forwarded_for("203.0.113.50:1234");
        assert_eq!(parsed, Some(("203.0.113.50".parse().unwrap(), Some(1234))));
    }

    #[test]
    fn test_parse_forwarded_for_ipv4_with_zero_port() {
        // Port 0 is not a real port — normalized to None.
        let parsed = parse_forwarded_for("203.0.113.50:0");
        assert_eq!(parsed, Some(("203.0.113.50".parse().unwrap(), None)));
    }

    #[test]
    fn test_parse_forwarded_for_ipv6_bracketed() {
        let parsed = parse_forwarded_for("[2001:db8::1]");
        assert_eq!(parsed, Some(("2001:db8::1".parse().unwrap(), None)));
    }

    #[test]
    fn test_parse_forwarded_for_ipv6_bracketed_with_port() {
        let parsed = parse_forwarded_for("[2001:db8::1]:8080");
        assert_eq!(parsed, Some(("2001:db8::1".parse().unwrap(), Some(8080))));
    }

    #[test]
    fn test_parse_forwarded_for_ipv6_bare() {
        let parsed = parse_forwarded_for("2001:db8::1");
        assert_eq!(parsed, Some(("2001:db8::1".parse().unwrap(), None)));
    }

    #[test]
    fn test_parse_forwarded_for_invalid() {
        let parsed = parse_forwarded_for("not-an-ip");
        assert_eq!(parsed, None);
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

    /// Build a forwarding chain of portless nodes (the XFF shape) from IP strings.
    fn chain(ips: &[&str]) -> Vec<(IpAddr, Option<u16>)> {
        ips.iter().map(|s| (s.parse().unwrap(), None)).collect()
    }

    fn ip(s: &str) -> (IpAddr, Option<u16>) {
        (s.parse().unwrap(), None)
    }

    #[test]
    fn test_extract_client_ip_single_untrusted() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        assert_eq!(
            handler.extract_client_ip(&chain(&["203.0.113.50"])),
            Some(ip("203.0.113.50"))
        );
    }

    #[test]
    fn test_extract_client_ip_rightmost_non_trusted() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        // Chain: [client, proxy1(trusted), proxy2(trusted)]
        // rightmost non-trusted = client
        let nodes = chain(&["203.0.113.50", "10.0.0.1", "10.0.0.2"]);
        assert_eq!(handler.extract_client_ip(&nodes), Some(ip("203.0.113.50")));
    }

    #[test]
    fn test_extract_client_ip_multi_hop() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        // Chain: [client, untrusted-intermediate, trusted-proxy]
        // rightmost non-trusted = untrusted-intermediate (not the very first)
        let nodes = chain(&["203.0.113.50", "198.51.100.1", "10.0.0.1"]);
        assert_eq!(handler.extract_client_ip(&nodes), Some(ip("198.51.100.1")));
    }

    #[test]
    fn test_extract_client_ip_all_trusted_returns_leftmost() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let nodes = chain(&["10.0.0.5", "10.0.0.1", "10.0.0.2"]);
        assert_eq!(handler.extract_client_ip(&nodes), Some(ip("10.0.0.5")));
    }

    #[test]
    fn test_extract_client_ip_empty_chain() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        assert_eq!(handler.extract_client_ip(&[]), None);
    }

    #[test]
    fn test_extract_client_ip_preserves_port() {
        // The selected node's source port rides along with the IP.
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let nodes = vec![
            ("203.0.113.50".parse().unwrap(), Some(5555u16)),
            ("10.0.0.1".parse().unwrap(), None),
        ];
        assert_eq!(
            handler.extract_client_ip(&nodes),
            Some(("203.0.113.50".parse().unwrap(), Some(5555)))
        );
    }

    #[test]
    fn test_extract_client_ip_all_trusted_keeps_leftmost_port() {
        // When every node is trusted, the leftmost node is returned WITH its
        // port (the all-trusted fallback carries the port like any other).
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let nodes = vec![
            ("10.0.0.5".parse().unwrap(), Some(1111u16)),
            ("10.0.0.1".parse().unwrap(), None),
        ];
        assert_eq!(
            handler.extract_client_ip(&nodes),
            Some(("10.0.0.5".parse().unwrap(), Some(1111)))
        );
    }

    #[test]
    fn test_extract_client_ip_spoofed_prefix() {
        // Attacker appends a spoofed trusted IP at the start of XFF chain.
        // XFF: [spoofed(10.0.0.1), attacker(evil.ip), trusted-proxy(10.0.0.2)]
        // rightmost-non-trusted = attacker IP, not the spoofed one
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let nodes = chain(&["10.0.0.1", "203.0.113.99", "10.0.0.2"]);
        assert_eq!(handler.extract_client_ip(&nodes), Some(ip("203.0.113.99")));
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

    fn meta<'a>(event: &'a RequestReceived, key: &str) -> Option<&'a str> {
        event
            .metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn test_handler_xff_port() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "203.0.113.50"),
                ("x-forwarded-host", "example.com"),
                ("x-forwarded-port", "8443"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(meta(&event, "forwarded_port"), Some("8443"));
    }

    #[test]
    fn test_handler_xff_port_untrusted_peer_ignored() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event =
            make_event_with_headers(untrusted_addr(), vec![("x-forwarded-port", "8443")]);
        handler.handle(&mut event);
        assert_eq!(meta(&event, "forwarded_port"), None);
    }

    #[test]
    fn test_handler_xff_port_invalid_dropped() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        for bad in ["not-a-number", "0", "70000", "-1", "443,80"] {
            let mut event =
                make_event_with_headers(trusted_addr(), vec![("x-forwarded-port", bad)]);
            handler.handle(&mut event);
            assert_eq!(
                meta(&event, "forwarded_port"),
                None,
                "value {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn test_handler_xff_port_suffix_entry_ignored() {
        // X-Forwarded-For entries with a `:port` suffix are not parsed as IPs
        // (XFF carries bare addresses — matches nginx/Caddy `real_ip`). The lone
        // unparseable entry is dropped, so remote_addr is NOT rewritten and
        // stays the trusted peer.
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event =
            make_event_with_headers(trusted_addr(), vec![("x-forwarded-for", "1.2.3.4:5678")]);
        let peer = event.remote_addr;
        handler.handle(&mut event);
        assert_eq!(event.remote_addr, peer);
    }

    #[test]
    fn test_handler_xff_port_suffix_node_skipped_in_chain() {
        // A port-suffixed (unparseable) XFF node does not poison the chain: the
        // rightmost untrusted parseable address is still selected.
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![("x-forwarded-for", "1.2.3.4:5678, 203.0.113.50")],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        assert_eq!(event.remote_addr.port(), 0);
    }

    #[test]
    fn test_handler_forwarded_present_ignores_xff_port() {
        // When `Forwarded` is present, X-Forwarded-* (incl. -Port) are ignored.
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("forwarded", "for=203.0.113.50;host=example.com"),
                ("x-forwarded-port", "8443"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(meta(&event, "forwarded_port"), None);
        // Also confirm the Forwarded branch actually ran — otherwise this test
        // could false-pass if the branch were dropped and nothing was rewritten.
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        assert_eq!(meta(&event, "forwarded_host"), Some("example.com"));
    }

    #[test]
    fn test_handler_forwarded_for_port_sets_remote_port() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![("forwarded", "for=\"203.0.113.50:5555\"")],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.50".parse::<IpAddr>().unwrap()
        );
        assert_eq!(event.remote_addr.port(), 5555);
    }

    #[test]
    fn test_handler_forwarded_for_without_port_zeroes_remote_port() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        let mut event =
            make_event_with_headers(trusted_addr(), vec![("forwarded", "for=203.0.113.50")]);
        handler.handle(&mut event);
        assert_eq!(event.remote_addr.port(), 0);
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

    // ── Cloudflare ───────────────────────────────────────────────────────────

    const CF_EDGE: &str = "104.23.1.2";

    fn cf_header(value: &str) -> (&str, &str) {
        ("cf-connecting-ip", value)
    }

    #[test]
    fn test_handler_cdn_behind_private_lb_uses_cf_connecting_ip() {
        // Cloudflare → load balancer → ingress → OxPHP. The visitor in
        // X-Forwarded-For differs from CF-Connecting-IP — the shape a
        // Worker-written X-Forwarded-For would take — so the assertion says
        // which of the two the address came from. The ingress hop inside the
        // chain makes the walk skip own proxies past the peer.
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "192.0.2.66, 104.23.1.2, 10.0.0.9"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(event.remote_addr.port(), 0);
    }

    #[test]
    fn test_handler_cdn_behind_private_lb_without_cf_header_uses_xff() {
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![("x-forwarded-for", "198.51.100.7, 104.23.1.2")],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_cdn_forwarded_uses_cf_connecting_ip() {
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("forwarded", "for=192.0.2.66, for=104.23.1.2"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_cdn_bypass_ignores_cf_connecting_ip() {
        // The client reached the load balancer directly, which appended its
        // address; the forged CF-Connecting-IP must not be read.
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "203.0.113.9"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_cdn_bypass_with_forged_edge_ignores_cf_connecting_ip() {
        // The client wrote a Cloudflare address into X-Forwarded-For itself;
        // the load balancer appended the client's real address after it.
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "104.16.0.1, 203.0.113.9"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_unparseable_hop_stops_the_cloudflare_walk() {
        // The load balancer wrote the client's address with a port, which
        // does not parse; the Cloudflare address left of it is the client's
        // own. The walk cannot vouch for anything past an unparseable hop, so
        // it stops there and CF-Connecting-IP is not read. Rightmost-non-
        // trusted then sees only trusted parsed nodes and keeps the leftmost.
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "104.16.0.1, 203.0.113.9:4444"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "104.16.0.1".parse::<IpAddr>().unwrap()
        );
    }

    /// An event whose header `name` carries `lines` as separate lines; a line
    /// may hold bytes outside visible ASCII, which `&str` headers cannot.
    fn event_with_raw_lines(name: &'static str, lines: &[&[u8]]) -> RequestReceived {
        let mut event = make_event_with_headers(trusted_addr(), vec![cf_header("198.51.100.7")]);
        for line in lines {
            event
                .parts
                .headers
                .append(name, http::HeaderValue::from_bytes(line).unwrap());
        }
        event
    }

    #[test]
    fn test_handler_unreadable_xff_line_does_not_affect_a_forwarded_chain() {
        // The load balancer maintains Forwarded and recorded the Cloudflare
        // edge there; Cloudflare adds nothing to Forwarded, so the entry left
        // of it is the client's. A client-sent X-Forwarded-For line with a
        // non-ASCII byte must not keep CF-Connecting-IP from being read, or
        // the client would choose its address through the fallback.
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let mut event = event_with_raw_lines("x-forwarded-for", &[b"\x80"]);
        event.parts.headers.append(
            "forwarded",
            http::HeaderValue::from_static("for=6.6.6.6, for=104.16.0.1"),
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_unreadable_line_left_of_the_edge_does_not_stop_the_walk() {
        // The client sent lines of its own, one with a non-ASCII byte; the
        // load balancer added a line recording the Cloudflare edge. The walk
        // must still reach that record.
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let cases: [(&'static str, &[&[u8]]); 2] = [
            (
                "x-forwarded-for",
                &[b"6.6.6.6", b"\x80, 203.0.113.50", b"104.16.0.1"],
            ),
            (
                "forwarded",
                &[b"for=6.6.6.6", b"for=7.7.7.7\x80", b"for=104.16.0.1"],
            ),
        ];
        for (name, lines) in cases {
            let mut event = event_with_raw_lines(name, lines);
            handler.handle(&mut event);
            assert_eq!(
                event.remote_addr.ip(),
                "198.51.100.7".parse::<IpAddr>().unwrap(),
                "{name}"
            );
        }
    }

    #[test]
    fn test_chain_keeps_only_parsed_hops() {
        let a = ip("192.0.2.1");
        let b = (ip("192.0.2.2").0, Some(80));
        let c = ip("192.0.2.3");
        let chain: Chain = [Some(a), None, Some(b), Some(c)].into_iter().collect();
        assert_eq!(chain.hops, [a, b, c]);
        assert_eq!(chain.unbroken_tail(), [b, c]);

        // Hops that do not parse take no room, however many there are.
        let chain: Chain = std::iter::repeat_n(None, 10_000).collect();
        assert_eq!(chain.hops.capacity(), 0);
        assert!(chain.unbroken_tail().is_empty());
    }

    #[test]
    fn test_handler_cloudflare_range_also_listed_by_hand_is_still_an_edge() {
        // A config carried over from nginx may list Cloudflare's ranges and
        // name the keyword too; the listed range must not turn the edge into
        // one of the operator's own proxies.
        let handler = TrustedProxyHandler::new(make_config("private,104.16.0.0/13,cloudflare"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "192.0.2.66, 104.23.1.2"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_cloudflare_edge_as_direct_peer() {
        let handler = TrustedProxyHandler::new(make_config("cloudflare"));
        let peer = SocketAddr::new(CF_EDGE.parse().unwrap(), 443);
        let mut event = make_event_with_headers(peer, vec![cf_header("198.51.100.7")]);
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_unusable_cf_connecting_ip_falls_back_to_xff() {
        let handler = TrustedProxyHandler::new(make_config("private,cloudflare"));
        let cases: [&[(&str, &str)]; 2] = [
            &[cf_header("not-an-ip")],
            &[cf_header("198.51.100.7"), cf_header("198.51.100.8")],
        ];
        for cf in cases {
            let mut headers = vec![("x-forwarded-for", "192.0.2.66, 104.23.1.2")];
            headers.extend_from_slice(cf);
            let mut event = make_event_with_headers(trusted_addr(), headers);
            handler.handle(&mut event);
            assert_eq!(
                event.remote_addr.ip(),
                "192.0.2.66".parse::<IpAddr>().unwrap(),
                "{cf:?}"
            );
        }
    }

    #[test]
    fn test_handler_cloudflare_ranges_without_keyword_ignore_cf_connecting_ip() {
        // Listing a Cloudflare range by hand trusts it as a proxy; reading
        // CF-Connecting-IP is reserved for the `cloudflare` keyword.
        let handler = TrustedProxyHandler::new(make_config("private,104.16.0.0/13"));
        let mut event = make_event_with_headers(
            trusted_addr(),
            vec![
                ("x-forwarded-for", "192.0.2.66, 104.23.1.2"),
                cf_header("198.51.100.7"),
            ],
        );
        handler.handle(&mut event);
        assert_eq!(
            event.remote_addr.ip(),
            "192.0.2.66".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn test_handler_priority_is_minus_80() {
        let handler = TrustedProxyHandler::new(make_config("10.0.0.0/8"));
        assert_eq!(handler.priority(), -80);
    }
}
