pub const OVERRIDE: &str = "SLICC_CDP_PORTS";

pub fn catalog() -> Vec<u16> {
    catalog_from(overridden().as_deref())
}

pub fn testing() -> bool {
    overridden().as_deref().and_then(parse).is_some()
}

fn overridden() -> Option<String> {
    if cfg!(target_os = "wasi") {
        return None;
    }
    std::env::var(OVERRIDE).ok()
}

fn catalog_from(value: Option<&str>) -> Vec<u16> {
    value
        .and_then(parse)
        .unwrap_or_else(|| slicc_browsers::catalog().discovery.probe_ports)
}

pub fn parse(text: &str) -> Option<Vec<u16>> {
    let text = text.trim();
    let (from, to) = text.split_once('-').unwrap_or((text, text));
    let from: u16 = from.trim().parse().ok()?;
    let to: u16 = to.trim().parse().ok()?;
    if from == 0 || to < from {
        return None;
    }
    Some((from..=to).collect())
}

pub fn real_debug_port(host: &str, port: u16) -> bool {
    if !(9222..=9229).contains(&port) {
        return false;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
        }
        Err(_) => false,
    }
}

pub fn first_live(
    ports: &[u16],
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Option<String> {
    for port in ports {
        let version = format!("http://127.0.0.1:{port}/json/version");
        let Ok(body) = probe(&version) else {
            continue;
        };
        if crate::connect::websocket_url_from_version(&body).is_ok() {
            return Some(version);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_a_single_port_and_junk() {
        assert_eq!(parse("40100-40102"), Some(vec![40100, 40101, 40102]));
        assert_eq!(parse(" 40100 "), Some(vec![40100]));
        assert_eq!(parse("40102-40100"), None);
        assert_eq!(parse("0-3"), None);
        assert_eq!(parse("a-b"), None);
        assert_eq!(parse("1-x"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn without_the_override_the_catalog_ports_are_probed() {
        let real: Vec<u16> = (9222..=9229).collect();
        assert_eq!(catalog_from(None), real);
        assert_eq!(catalog_from(Some("junk")), real);
        assert_eq!(catalog_from(Some("9300-9302")), vec![9300, 9301, 9302]);
    }

    #[test]
    fn cargo_test_runs_with_the_harness_range() {
        assert_eq!(std::env::var(OVERRIDE).as_deref(), Ok("9300-9309"));
        assert!(testing());
        assert_eq!(catalog(), (9300..=9309).collect::<Vec<u16>>());
    }

    #[test]
    fn real_debug_ports_are_loopback_or_unspecified_9222_to_9229() {
        for (host, port) in [
            ("127.0.0.1", 9222),
            ("127.0.0.2", 9229),
            ("0.0.0.0", 9223),
            ("localhost", 9224),
            ("LOCALHOST.", 9224),
            ("app.localhost", 9225),
            ("::1", 9226),
            ("[::1]", 9226),
            ("::", 9227),
            ("::ffff:127.0.0.1", 9228),
            ("::ffff:0.0.0.0", 9228),
        ] {
            assert!(real_debug_port(host, port), "{host}:{port}");
        }
        for (host, port) in [
            ("127.0.0.1", 9230),
            ("127.0.0.1", 9221),
            ("93.184.216.34", 9222),
            ("example.com", 9222),
            ("::ffff:93.184.216.34", 9222),
            ("2001:db8::1", 9222),
        ] {
            assert!(!real_debug_port(host, port), "{host}:{port}");
        }
    }

    #[test]
    fn the_first_port_with_a_debugger_wins() {
        let mut seen = Vec::new();
        let found = first_live(&[40100, 40101, 40102, 40103], &mut |url| {
            seen.push(url.to_string());
            match url {
                "http://127.0.0.1:40100/json/version" => Err("refused\n".to_string()),
                "http://127.0.0.1:40101/json/version" => Ok("{}".to_string()),
                "http://127.0.0.1:40102/json/version" => Ok(
                    r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:40102/devtools/browser/b"}"#
                        .to_string(),
                ),
                _ => panic!("probed past the first live port: {url}"),
            }
        });
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:40102/json/version"));
        assert_eq!(seen.len(), 3);
        assert_eq!(first_live(&[40100], &mut |_| Err("HTTP 404\n".to_string())), None);
    }
}
