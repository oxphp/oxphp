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
        let mut networks = Vec::new();
        for part in trimmed.split(',') {
            let cidr = part.trim();
            if cidr.is_empty() {
                continue;
            }
            // Try CIDR first; fall back to bare IP → host route (/32 or /128).
            let net: IpNet = if let Ok(n) = cidr.parse::<IpNet>() {
                n
            } else if let Ok(ip) = cidr.parse::<IpAddr>() {
                match ip {
                    IpAddr::V4(a) => IpNet::V4(Ipv4Net::new(a, 32).unwrap()),
                    IpAddr::V6(a) => IpNet::V6(Ipv6Net::new(a, 128).unwrap()),
                }
            } else {
                // Neither CIDR nor plain IP — parse as IpNet to produce a consistent error.
                cidr.parse::<IpNet>()
                    .map_err(|e| format!("invalid CIDR in TRUSTED_PROXIES: {:?}: {}", cidr, e))?
            };
            networks.push(net);
        }
        if networks.is_empty() {
            return Err("TRUSTED_PROXIES is set but contains no valid CIDRs".to_string());
        }
        Ok(networks)
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

#[cfg(test)]
mod tests {
    use super::*;

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
