//! Which proxy a request goes through: `https_proxy` / `http_proxy` (either
//! case, then `all_proxy`), unless the host is in `no_proxy`.

use std::net::IpAddr;

use super::url::Url;
use super::Error;

/// A plain-HTTP proxy at `host:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyAddr {
    pub host: String,
    pub port: u16,
}

impl ProxyAddr {
    pub fn parse(proxy: &str) -> Result<ProxyAddr, Error> {
        let bad = |why: &str| Error::BadProxy(format!("`{proxy}`: {why}"));
        let rest = match proxy.split_once("://") {
            None => proxy,
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
            Some((scheme, _)) => {
                return Err(bad(&format!(
                    "{scheme} proxies are not supported, only http:// (this client has no TLS or SOCKS)"
                )))
            }
        };
        let url = Url::parse(&format!("http://{rest}")).map_err(|_| bad("not host:port"))?;
        Ok(ProxyAddr {
            port: url.port.unwrap_or(80),
            host: url.host,
        })
    }
}

fn env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()))
}

/// The proxy the environment names for `url`, if any.
pub(crate) fn from_env(url: &Url) -> Result<Option<ProxyAddr>, Error> {
    let names: &[&str] = if url.scheme == "https" {
        &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    } else {
        &["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
    };
    let Some(proxy) = env(names) else {
        return Ok(None);
    };
    let list = env(&["no_proxy", "NO_PROXY"]).unwrap_or_default();
    if no_proxy(&list, &url.host, url.port_or_default()) {
        return Ok(None);
    }
    ProxyAddr::parse(&proxy).map(Some)
}

/// Whether `no_proxy` (comma- or space-separated: `*`, names that also
/// match their subdomains, with or without a leading dot, addresses,
/// IPv4 CIDR blocks, any of them with `:port`) exempts `host`.
pub(crate) fn no_proxy(list: &str, host: &str, port: u16) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let ip = host.parse::<IpAddr>().ok();
    list.split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .any(|entry| {
            if entry == "*" {
                return true;
            }
            let entry = entry.to_ascii_lowercase();
            if let Some((net, bits)) = entry.split_once('/') {
                return match (ip, net.parse::<IpAddr>(), bits.parse::<u32>()) {
                    (Some(IpAddr::V4(ip)), Ok(IpAddr::V4(net)), Ok(bits)) if bits <= 32 => {
                        let mask = if bits == 0 {
                            0
                        } else {
                            u32::MAX << (32 - bits)
                        };
                        u32::from(ip) & mask == u32::from(net) & mask
                    }
                    (Some(IpAddr::V6(ip)), Ok(IpAddr::V6(net)), Ok(bits)) if bits <= 128 => {
                        let mask = if bits == 0 {
                            0
                        } else {
                            u128::MAX << (128 - bits)
                        };
                        u128::from(ip) & mask == u128::from(net) & mask
                    }
                    _ => false,
                };
            }
            let unbracketed = entry.trim_start_matches('[').trim_end_matches(']');
            let (name, entry_port) = if unbracketed.parse::<IpAddr>().is_ok() {
                (unbracketed.to_string(), None)
            } else {
                match entry.rsplit_once(':') {
                    Some((n, p)) if p.parse::<u16>().is_ok() => (
                        n.trim_start_matches('[').trim_end_matches(']').to_string(),
                        p.parse::<u16>().ok(),
                    ),
                    _ => (entry.clone(), None),
                }
            };
            if entry_port.is_some_and(|p| p != port) {
                return false;
            }
            if let (Some(ip), Ok(other)) = (ip, name.parse::<IpAddr>()) {
                return ip == other;
            }
            let name = name
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .trim_end_matches('.');
            !name.is_empty() && (host == name || host.ends_with(&format!(".{name}")))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_no_proxy_covers_loopback() {
        let list = "localhost,.localhost,127.0.0.1,127.0.0.0/8";
        assert!(no_proxy(list, "localhost", 80));
        assert!(no_proxy(list, "app.localhost", 80));
        assert!(no_proxy(list, "127.0.0.1", 3128));
        assert!(no_proxy(list, "127.4.5.6", 1));
        assert!(!no_proxy(list, "128.0.0.1", 1));
        assert!(!no_proxy(list, "example.com", 443));
        assert!(!no_proxy(list, "notlocalhost", 80));
    }

    #[test]
    fn names_ports_and_wildcards() {
        assert!(no_proxy("*", "anything", 1));
        assert!(no_proxy("example.com", "api.example.com", 443));
        assert!(no_proxy(".example.com", "example.com", 443));
        assert!(no_proxy("*.example.com", "a.example.com", 443));
        assert!(no_proxy("a.test:8080", "a.test", 8080));
        assert!(!no_proxy("a.test:8080", "a.test", 80));
        assert!(no_proxy("::1", "::1", 80));
        assert!(no_proxy("[::1]:80", "::1", 80));
        assert!(no_proxy("fd00::/8", "fd12::1", 80));
        assert!(!no_proxy("", "a", 80));
        assert!(no_proxy(" x.test  , y.test", "y.test", 80));
    }

    #[test]
    fn proxy_urls() {
        let p = |s| ProxyAddr::parse(s);
        assert_eq!(
            p("http://127.0.0.1:3128").unwrap(),
            ProxyAddr {
                host: "127.0.0.1".into(),
                port: 3128
            }
        );
        assert_eq!(
            p("http://u:p@localhost:8080/").unwrap(),
            ProxyAddr {
                host: "localhost".into(),
                port: 8080
            }
        );
        assert_eq!(
            p("proxy.test").unwrap(),
            ProxyAddr {
                host: "proxy.test".into(),
                port: 80
            }
        );
        assert!(p("https://127.0.0.1:3128").is_err());
        assert!(p("socks5://127.0.0.1:1080").is_err());
    }
}
