use std::net::IpAddr;

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

/// Configuration for trusted reverse proxies.
/// When set, the server inspects `Forwarded` / `X-Forwarded-*` headers
/// from connections whose peer IP falls within a trusted CIDR range.
#[derive(Debug, Clone)]
pub struct TrustedProxyConfig {
    networks: Vec<IpNet>,
}

impl TrustedProxyConfig {
    /// Parse `TRUSTED_PROXIES` env var.
    /// Accepts comma-separated CIDRs or the special value `"private"`.
    /// Returns `None` if the variable is unset or empty.
    /// Returns `Err` if the value contains unparseable CIDRs.
    pub fn from_env() -> Result<Option<Self>, String> {
        let val = match std::env::var("TRUSTED_PROXIES") {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(None),
        };
        let networks = Self::parse(&val)?;
        Ok(Some(Self { networks }))
    }

    /// Construct from a spec string directly (for testing and programmatic use).
    pub fn from_spec(spec: &str) -> Self {
        Self {
            networks: Self::parse(spec).expect("invalid trusted proxy spec"),
        }
    }

    /// Parse a trusted proxies spec string into a list of networks.
    fn parse(val: &str) -> Result<Vec<IpNet>, String> {
        let trimmed = val.trim();
        if trimmed.eq_ignore_ascii_case("private") {
            return Ok(Self::private_ranges());
        }
        parse_cidr_list(trimmed, "TRUSTED_PROXIES")
    }

    /// RFC-1918 + loopback + link-local for both IPv4 and IPv6.
    fn private_ranges() -> Vec<IpNet> {
        [
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "::1/128",
            "fc00::/7",
            "fe80::/10",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect()
    }

    /// Check whether an IP address belongs to any trusted network.
    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        self.networks.iter().any(|net| net.contains(&ip))
    }
}

/// Parse a comma-separated list of CIDRs / bare IPs into networks.
/// Bare IPs become host routes (`/32` or `/128`). `var_name` appears only in
/// error messages. Returns `Err` if any entry is unparseable or the list is empty.
pub fn parse_cidr_list(val: &str, var_name: &str) -> Result<Vec<IpNet>, String> {
    let mut networks = Vec::new();
    for part in val.trim().split(',') {
        let cidr = part.trim();
        if cidr.is_empty() {
            continue;
        }
        // Try CIDR first; fall back to bare IP → host route (/32 or /128).
        // Keep the CIDR parse error to report *why* an entry was rejected.
        let net: IpNet = match cidr.parse::<IpNet>() {
            Ok(n) => n,
            Err(e) => match cidr.parse::<IpAddr>() {
                Ok(IpAddr::V4(a)) => IpNet::V4(Ipv4Net::new(a, 32).unwrap()),
                Ok(IpAddr::V6(a)) => IpNet::V6(Ipv6Net::new(a, 128).unwrap()),
                Err(_) => return Err(format!("invalid CIDR in {var_name}: {cidr:?}: {e}")),
            },
        };
        networks.push(net);
    }
    if networks.is_empty() {
        return Err(format!("{var_name} is set but contains no valid CIDRs"));
    }
    Ok(networks)
}

/// Network allow-list for the internal server (`INTERNAL_ALLOW_IPS`).
#[derive(Debug, Clone)]
pub struct IpAllowList {
    networks: Vec<IpNet>,
}

impl IpAllowList {
    /// Parse `INTERNAL_ALLOW_IPS`. `None` if unset or empty; `Err` on a
    /// malformed list (a hard startup error at the call site).
    pub fn from_env() -> Result<Option<Self>, String> {
        let val = match std::env::var("INTERNAL_ALLOW_IPS") {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return Ok(None),
        };
        Ok(Some(Self {
            networks: parse_cidr_list(&val, "INTERNAL_ALLOW_IPS")?,
        }))
    }

    /// Build from a spec string directly (tests / programmatic use).
    #[cfg(test)]
    pub fn from_spec(spec: &str) -> Self {
        Self {
            networks: parse_cidr_list(spec, "INTERNAL_ALLOW_IPS")
                .expect("invalid INTERNAL_ALLOW_IPS spec"),
        }
    }

    /// Whether `ip` is inside any allowed network. IPv4-mapped IPv6 peers are
    /// canonicalized so they match IPv4 CIDRs.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.networks.iter().any(|net| net.contains(&ip))
    }
}

/// How exposed a bound internal-server address is, for the startup warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindExposure {
    /// Loopback (127.0.0.0/8, ::1) — not reachable off-host.
    Loopback,
    /// RFC1918 / ULA / link-local — a deliberate internal interface.
    Private,
    /// 0.0.0.0 / :: (all interfaces) or a public address — reachable off-host.
    Exposed,
}

/// Classify a bound IP for the non-loopback startup warning.
pub fn classify_bind_exposure(ip: IpAddr) -> BindExposure {
    if ip.is_loopback() {
        BindExposure::Loopback
    } else if ip.is_unspecified() {
        BindExposure::Exposed
    } else if TrustedProxyConfig::private_ranges()
        .iter()
        .any(|net| net.contains(&ip))
    {
        BindExposure::Private
    } else {
        BindExposure::Exposed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cidr_list_invalid_is_error() {
        assert!(parse_cidr_list("not-a-cidr", "INTERNAL_ALLOW_IPS").is_err());
    }

    #[test]
    fn test_parse_cidr_list_error_includes_underlying_cause() {
        // The error appends the underlying parse cause after the quoted entry,
        // so an operator sees *why* it failed (e.g. an out-of-range prefix).
        let err = parse_cidr_list("10.0.0.0/33", "TRUSTED_PROXIES").unwrap_err();
        assert!(err.contains("invalid CIDR"));
        assert!(err.contains("10.0.0.0/33"));
        // Two colons: after the var name and after the quoted entry (before the cause).
        assert!(err.matches(':').count() >= 2, "cause missing: {err}");
    }

    #[test]
    fn test_parse_cidr_list_empty_is_error() {
        assert!(parse_cidr_list("  ,  ", "INTERNAL_ALLOW_IPS").is_err());
    }

    #[test]
    fn test_allowlist_contains_ipv4() {
        let allow = IpAllowList::from_spec("10.0.0.0/8, 192.168.1.5");
        assert!(allow.contains("10.1.2.3".parse().unwrap()));
        assert!(allow.contains("192.168.1.5".parse().unwrap()));
        assert!(!allow.contains("203.0.113.7".parse().unwrap()));
    }

    #[test]
    fn test_allowlist_contains_ipv4_mapped_ipv6() {
        // A peer arriving on a `::` socket appears as ::ffff:10.0.0.5
        let allow = IpAllowList::from_spec("10.0.0.0/8");
        let mapped: std::net::IpAddr = "::ffff:10.0.0.5".parse().unwrap();
        assert!(allow.contains(mapped));
    }

    #[test]
    fn test_classify_bind_exposure() {
        use std::net::IpAddr;
        assert_eq!(
            classify_bind_exposure("127.0.0.1".parse::<IpAddr>().unwrap()),
            BindExposure::Loopback
        );
        assert_eq!(
            classify_bind_exposure("::1".parse::<IpAddr>().unwrap()),
            BindExposure::Loopback
        );
        assert_eq!(
            classify_bind_exposure("0.0.0.0".parse::<IpAddr>().unwrap()),
            BindExposure::Exposed
        );
        assert_eq!(
            classify_bind_exposure("::".parse::<IpAddr>().unwrap()),
            BindExposure::Exposed
        );
        assert_eq!(
            classify_bind_exposure("10.4.5.6".parse::<IpAddr>().unwrap()),
            BindExposure::Private
        );
        assert_eq!(
            classify_bind_exposure("192.168.0.9".parse::<IpAddr>().unwrap()),
            BindExposure::Private
        );
        assert_eq!(
            classify_bind_exposure("203.0.113.7".parse::<IpAddr>().unwrap()),
            BindExposure::Exposed
        );
    }

    #[test]
    fn test_parse_single_cidr() {
        let nets = TrustedProxyConfig::parse("10.0.0.0/8").unwrap();
        assert_eq!(nets.len(), 1);
    }

    #[test]
    fn test_parse_multiple_cidrs() {
        let nets = TrustedProxyConfig::parse("10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16").unwrap();
        assert_eq!(nets.len(), 3);
    }

    #[test]
    fn test_parse_private() {
        let nets = TrustedProxyConfig::parse("private").unwrap();
        assert_eq!(nets.len(), 8);
    }

    #[test]
    fn test_parse_private_case_insensitive() {
        let nets = TrustedProxyConfig::parse("PRIVATE").unwrap();
        assert_eq!(nets.len(), 8);
    }

    #[test]
    fn test_parse_invalid_cidr() {
        let result = TrustedProxyConfig::parse("not-a-cidr");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid CIDR"));
    }

    #[test]
    fn test_parse_empty_after_trim() {
        let result = TrustedProxyConfig::parse("  ,  ,  ");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no valid CIDRs"));
    }

    #[test]
    fn test_parse_ipv6_cidr() {
        let nets = TrustedProxyConfig::parse("::1/128, fc00::/7").unwrap();
        assert_eq!(nets.len(), 2);
    }

    #[test]
    fn test_parse_single_ip_becomes_host_cidr() {
        let nets = TrustedProxyConfig::parse("10.0.0.1").unwrap();
        assert_eq!(nets.len(), 1);
    }

    #[test]
    fn test_is_trusted_in_range() {
        let config = TrustedProxyConfig::from_spec("10.0.0.0/8");
        assert!(config.is_trusted("10.1.2.3".parse().unwrap()));
        assert!(config.is_trusted("10.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_trusted_not_in_range() {
        let config = TrustedProxyConfig::from_spec("10.0.0.0/8");
        assert!(!config.is_trusted("11.0.0.1".parse().unwrap()));
        assert!(!config.is_trusted("192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_is_trusted_ipv6() {
        let config = TrustedProxyConfig::from_spec("fc00::/7");
        assert!(config.is_trusted("fd00::1".parse().unwrap()));
        assert!(!config.is_trusted("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn test_is_trusted_private() {
        let config = TrustedProxyConfig::from_spec("private");
        assert!(config.is_trusted("10.0.0.1".parse().unwrap()));
        assert!(config.is_trusted("172.16.5.5".parse().unwrap()));
        assert!(config.is_trusted("192.168.1.1".parse().unwrap()));
        assert!(config.is_trusted("127.0.0.1".parse().unwrap()));
        assert!(config.is_trusted("::1".parse().unwrap()));
        assert!(config.is_trusted("fd00::1".parse().unwrap()));
        assert!(!config.is_trusted("8.8.8.8".parse().unwrap()));
        assert!(!config.is_trusted("203.0.113.50".parse().unwrap()));
    }
}
