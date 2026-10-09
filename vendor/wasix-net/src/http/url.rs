//! Just enough URL handling for HTTP requests and redirects.

use super::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Url {
    pub scheme: String,
    /// Lower-case, without IPv6 brackets.
    pub host: String,
    pub port: Option<u16>,
    /// Path and query, starting with `/`; the fragment is dropped.
    pub path: String,
}

fn bad(url: &str, why: &str) -> Error {
    Error::BadUrl(format!("`{url}`: {why}"))
}

fn has_scheme(s: &str) -> bool {
    match s.find(':') {
        Some(i) if i > 0 => {
            let scheme = &s[..i];
            scheme.as_bytes()[0].is_ascii_alphabetic()
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        }
        _ => false,
    }
}

impl Url {
    pub fn parse(input: &str) -> Result<Url, Error> {
        let s = input.trim();
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| bad(input, "not an absolute URL"))?;
        if !has_scheme(&format!("{scheme}:")) {
            return Err(bad(input, "bad scheme"));
        }
        let scheme = scheme.to_ascii_lowercase();
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (host, after) = v6
                .split_once(']')
                .ok_or_else(|| bad(input, "bad IPv6 host"))?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':').ok_or_else(|| bad(input, "bad port"))?),
            };
            (host, port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        if host.is_empty() {
            return Err(bad(input, "no host"));
        }
        let port = match port {
            None | Some("") => None,
            Some(p) => Some(p.parse::<u16>().map_err(|_| bad(input, "bad port"))?),
        };
        let tail = tail.split('#').next().unwrap_or_default();
        let path = if tail.starts_with('/') {
            tail.to_string()
        } else {
            format!("/{tail}")
        };
        if path.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(bad(input, "whitespace or control characters in the path"));
        }
        Ok(Url {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            path,
        })
    }

    pub fn default_port(&self) -> Option<u16> {
        match self.scheme.as_str() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        }
    }

    pub fn port_or_default(&self) -> u16 {
        self.port.or(self.default_port()).unwrap_or(80)
    }

    /// `host[:port]` as the Host header carries it.
    pub fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(p) if Some(p) != self.default_port() => format!("{host}:{p}"),
            _ => host,
        }
    }

    pub fn absolute(&self) -> String {
        format!("{}://{}{}", self.scheme, self.authority(), self.path)
    }

    /// Resolves a `Location` against this URL.
    pub fn join(&self, location: &str) -> Result<Url, Error> {
        let location = location.trim();
        if has_scheme(location) && location.contains("://") {
            return Url::parse(location);
        }
        if let Some(rest) = location.strip_prefix("//") {
            return Url::parse(&format!("{}://{rest}", self.scheme));
        }
        let location = location.split('#').next().unwrap_or_default();
        let base_path = self.path.split('?').next().unwrap_or("/");
        let path = if location.is_empty() {
            self.path.clone()
        } else if location.starts_with('/') {
            location.to_string()
        } else if location.starts_with('?') {
            format!("{base_path}{location}")
        } else {
            let dir = &base_path[..=base_path.rfind('/').unwrap_or(0)];
            format!("{dir}{location}")
        };
        let mut joined = self.clone();
        joined.path = normalize(&path);
        if joined.path.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err(bad(
                location,
                "whitespace or control characters in the path",
            ));
        }
        Ok(joined)
    }
}

/// Removes `.` and `..` segments from the path (RFC 3986 5.2.4).
fn normalize(path: &str) -> String {
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    for (i, seg) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *seg {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                out.pop();
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let mut result = format!("/{}", out.join("/"));
    if let Some(q) = query {
        result.push('?');
        result.push_str(q);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_ports_and_paths() {
        let u = Url::parse("HTTPS://user:pw@Example.COM:8443/a/b?q=1#frag").unwrap();
        assert_eq!(
            (u.scheme.as_str(), u.host.as_str(), u.port),
            ("https", "example.com", Some(8443))
        );
        assert_eq!(u.path, "/a/b?q=1");
        assert_eq!(u.absolute(), "https://example.com:8443/a/b?q=1");
        let u = Url::parse("http://127.0.0.1?x").unwrap();
        assert_eq!(
            (u.authority(), u.path.as_str()),
            ("127.0.0.1".to_string(), "/?x")
        );
        let u = Url::parse("http://[::1]:80/").unwrap();
        assert_eq!(
            (u.host.as_str(), u.authority()),
            ("::1", "[::1]".to_string())
        );
        assert!(Url::parse("example.com/x").is_err());
        assert!(Url::parse("http:///x").is_err());
        assert!(Url::parse("http://h:99999/").is_err());
        assert!(Url::parse("http://h/a b").is_err());
    }

    #[test]
    fn joins_locations() {
        let base = Url::parse("http://h:81/a/b/c?q").unwrap();
        let j = |l: &str| base.join(l).unwrap().absolute();
        assert_eq!(j("https://o/x"), "https://o/x");
        assert_eq!(j("//o2/y"), "http://o2/y");
        assert_eq!(j("/abs?z"), "http://h:81/abs?z");
        assert_eq!(j("d"), "http://h:81/a/b/d");
        assert_eq!(j("../d"), "http://h:81/a/d");
        assert_eq!(j("./"), "http://h:81/a/b/");
        assert_eq!(j("?r"), "http://h:81/a/b/c?r");
        assert_eq!(j("../../../../x"), "http://h:81/x");
    }
}
