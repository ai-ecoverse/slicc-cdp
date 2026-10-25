use crate::body::resolve_path;
use crate::parse::Stop;
use crate::request::PreparedRequest;
use ed25519_compact::{KeyPair, Seed, SecretKey};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LABEL: &str = "sig1";
pub const LIFETIME_SECONDS: u64 = 60;
pub const HEADERS: [&str; 3] = ["Signature", "Signature-Input", "Signature-Agent"];

pub struct Signer {
    secret: SecretKey,
    keyid: String,
    agent: Option<String>,
    legacy: bool,
}

pub struct Signed {
    pub input: String,
    pub signature: String,
    pub agent: Option<String>,
}

pub struct Moment<'a> {
    pub label: &'a str,
    pub member: &'a str,
    pub created: u64,
    pub expires: u64,
    pub nonce: &'a str,
}

pub fn load(path: &str, agent: Option<&str>, legacy: bool) -> Result<Signer, Stop> {
    let text = std::fs::read_to_string(resolve_path(path))
        .map_err(|_| Stop::new(format!("curlwright: cannot read --sign-key {path}"), 26))?;
    let agent = agent.map(signature_agent).transpose().map_err(Stop::usage)?;
    from_jwk(&text, agent, legacy).map_err(|reason| Stop::usage(format!("curlwright: --sign-key {path} {reason}")))
}

pub fn from_jwk(text: &str, agent: Option<String>, legacy: bool) -> Result<Signer, String> {
    let jwk: Value = serde_json::from_str(text).map_err(|_| "is not a JSON Web Key".to_string())?;
    if !jwk.is_object() {
        return Err("is not a JSON Web Key".into());
    }
    if jwk["kty"] != "OKP" || jwk["crv"] != "Ed25519" {
        return Err("is not an Ed25519 key: Web Bot Auth signing needs kty \"OKP\" and crv \"Ed25519\"".into());
    }
    let Some(d) = jwk["d"].as_str() else {
        return Err("has no private part \"d\"; signing needs the private key".into());
    };
    let seed: [u8; 32] = base64url_decode(d).ok().and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "has an invalid \"d\": expected 32 bytes of base64url".to_string())?;
    let pair = KeyPair::try_from_seed(Seed::new(seed)).map_err(|_| "has an invalid \"d\"".to_string())?;
    if let Some(x) = jwk.get("x") {
        let given = x.as_str().and_then(|x| base64url_decode(x).ok());
        if given.as_deref() != Some(&pair.pk[..]) {
            return Err("has an \"x\" that is not the public key of its \"d\"".into());
        }
    }
    Ok(Signer { keyid: thumbprint(&pair.pk[..]), secret: pair.sk, agent, legacy })
}

pub fn signature_agent(value: &str) -> Result<String, String> {
    let plain = !value.is_empty() && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'"' && byte != b'\\');
    let https = url::Url::parse(value).ok().is_some_and(|url| url.scheme() == "https" && url.host_str().is_some_and(|host| !host.is_empty()));
    if plain && https {
        Ok(format!("\"{value}\""))
    } else {
        Err("curlwright: --signature-agent must be an https URL in printable ASCII".into())
    }
}

pub fn thumbprint(public: &[u8]) -> String {
    let members = format!("{{\"crv\":\"Ed25519\",\"kty\":\"OKP\",\"x\":\"{}\"}}", base64url_encode(public));
    base64url_encode(&hex_bytes(&cdp_client::hash::sha256_hex(members.as_bytes())))
}

pub fn authority(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|_| "curlwright: cannot sign a request without an absolute http(s) URL\n".to_string())?;
    let host = parsed.host_str().filter(|host| !host.is_empty() && matches!(parsed.scheme(), "http" | "https"));
    let host = host.ok_or_else(|| "curlwright: cannot sign a request without an absolute http(s) URL\n".to_string())?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

impl Signer {
    fn base(&self, authority: &str, moment: &Moment) -> (String, String, Option<String>) {
        let mut components = vec!["\"@authority\"".to_string()];
        let mut lines = vec![format!("\"@authority\": {authority}")];
        let agent = self.agent.as_ref().map(|value| {
            let (component, header) = if self.legacy {
                ("\"signature-agent\"".to_string(), value.clone())
            } else {
                (format!("\"signature-agent\";key=\"{}\"", moment.member), format!("{}={value}", moment.member))
            };
            lines.push(format!("{component}: {value}"));
            components.push(component);
            header
        });
        let params = format!(
            "({});created={};keyid=\"{}\";alg=\"ed25519\";expires={};nonce=\"{}\";tag=\"web-bot-auth\"",
            components.join(" "), moment.created, self.keyid, moment.expires, moment.nonce
        );
        lines.push(format!("\"@signature-params\": {params}"));
        (lines.join("\n"), params, agent)
    }

    pub fn sign(&self, authority: &str, moment: &Moment) -> Signed {
        let (base, params, agent) = self.base(authority, moment);
        let signature = self.secret.sign(base.as_bytes(), None);
        Signed {
            input: format!("{}={params}", moment.label),
            signature: format!("{}=:{}:", moment.label, cdp_client::net::base64_encode(&signature[..])),
            agent,
        }
    }

    pub fn apply(&self, request: &mut PreparedRequest) -> Result<(), String> {
        let authority = authority(&request.url)?;
        let created = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "curlwright: the clock is before 1970\n".to_string())?.as_secs();
        let mut entropy = [0u8; 64];
        getrandom::fill(&mut entropy).map_err(|_| "curlwright: no entropy for the signature nonce\n".to_string())?;
        let nonce = cdp_client::net::base64_encode(&entropy);
        let moment = Moment { label: LABEL, member: LABEL, created, expires: created + LIFETIME_SECONDS, nonce: &nonce };
        let signed = self.sign(&authority, &moment);
        request.headers.retain(|name, _| !HEADERS.iter().any(|header| header.eq_ignore_ascii_case(name)));
        request.headers.insert("Signature-Input".into(), Value::String(signed.input));
        request.headers.insert("Signature".into(), Value::String(signed.signature));
        if let Some(agent) = signed.agent {
            request.headers.insert("Signature-Agent".into(), Value::String(agent));
        }
        Ok(())
    }
}

pub fn base64url_encode(bytes: &[u8]) -> String {
    cdp_client::net::base64_encode(bytes).trim_end_matches('=').replace('+', "-").replace('/', "_")
}

pub fn base64url_decode(text: &str) -> Result<Vec<u8>, String> {
    if !text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_') || text.len() % 4 == 1 {
        return Err("invalid base64url\n".into());
    }
    let mut standard = text.replace('-', "+").replace('_', "/");
    while standard.len() % 4 != 0 {
        standard.push('=');
    }
    cdp_client::net::base64_decode(&standard)
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).filter_map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("../../../test/integration/fixtures/web-bot-auth-vectors.json");

    fn vectors() -> Vec<Value> {
        serde_json::from_str::<Value>(VECTORS).unwrap()["vectors"].as_array().unwrap().clone()
    }

    #[test]
    fn cloudflare_vectors_sign_to_the_published_signature_input_and_signature() {
        let all = vectors();
        assert_eq!(all.len(), 4);
        for vector in all {
            let raw = vector["signature_agent"].as_str();
            let member = vector["signature_agent_key"].as_str();
            let legacy = raw.is_some() && member.is_none();
            let agent = raw.map(|raw| match raw.split_once('=') {
                Some((_, value)) if !legacy => value.to_string(),
                _ => raw.to_string(),
            });
            let signer = from_jwk(&vector["key"].to_string(), agent, legacy).unwrap();
            let label = vector["label"].as_str().unwrap();
            let moment = Moment {
                label,
                member: member.unwrap_or(label),
                created: vector["created_ms"].as_u64().unwrap() / 1000,
                expires: vector["expires_ms"].as_u64().unwrap() / 1000,
                nonce: vector["nonce"].as_str().unwrap(),
            };
            let target = vector["target_url"].as_str().unwrap();
            let signed = signer.sign(&authority(target).unwrap(), &moment);
            let (base, _, _) = signer.base(&authority(target).unwrap(), &moment);
            assert_eq!(signed.input, vector["signature_input"].as_str().unwrap());
            assert_eq!(signed.signature, vector["signature"].as_str().unwrap());
            assert_eq!(signed.agent.as_deref(), raw);
            assert_eq!(base, vector["signature_base"].as_str().unwrap());
            assert_eq!(signer.keyid, "poqkLGiymh_W0uP6PZFw-dvez3QJT5SolqXBCW38r0U");
            let public = ed25519_compact::PublicKey::from_slice(&base64url_decode(vector["key"]["x"].as_str().unwrap()).unwrap()).unwrap();
            let bytes = cdp_client::net::base64_decode(signed.signature.split(':').nth(1).unwrap()).unwrap();
            public.verify(base.as_bytes(), &ed25519_compact::Signature::from_slice(&bytes).unwrap()).unwrap();
        }
    }

    #[test]
    fn rfc_8037_thumbprint_matches_its_published_example() {
        let x = base64url_decode("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo").unwrap();
        assert_eq!(thumbprint(&x), "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k");
    }

    #[test]
    fn bad_keys_are_refused_without_echoing_the_key() {
        let secret = "n4Ni-HpISpVObnQMW0wOhCKROaIKqKtW_2ZYb2p9KcU";
        let cases = [
            ("not json".to_string(), "is not a JSON Web Key"),
            ("[1]".to_string(), "is not a JSON Web Key"),
            (format!("{{\"kty\":\"RSA\",\"d\":\"{secret}\"}}"), "is not an Ed25519 key"),
            (format!("{{\"kty\":\"OKP\",\"crv\":\"X25519\",\"d\":\"{secret}\"}}"), "is not an Ed25519 key"),
            ("{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"x\":\"JrQLj5P_89iXES9-vFgrIy29clF9CC_oPPsw3c5D0bs\"}".to_string(), "has no private part"),
            ("{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"n4Ni+HpI\"}".to_string(), "has an invalid \"d\""),
            ("{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"AAAA\"}".to_string(), "has an invalid \"d\""),
            ("{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\"}".to_string(), "has an invalid \"d\""),
            (format!("{{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"{secret}\",\"x\":\"11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo\"}}"), "has an \"x\" that is not"),
            (format!("{{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"{secret}\",\"x\":7}}"), "has an \"x\" that is not"),
        ];
        for (text, reason) in cases {
            let Err(message) = from_jwk(&text, None, false) else { panic!("{reason}") };
            assert!(message.starts_with(reason), "{message}");
            assert!(!message.contains(secret), "{message}");
        }
        let signer = from_jwk(&format!("{{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"{secret}\"}}"), None, false).unwrap();
        assert_eq!(signer.keyid, "poqkLGiymh_W0uP6PZFw-dvez3QJT5SolqXBCW38r0U");
    }

    #[test]
    fn signature_agent_must_be_a_plain_https_url() {
        assert_eq!(signature_agent("https://signature-agent.test").unwrap(), "\"https://signature-agent.test\"");
        for value in ["", "http://signature-agent.test", "https://a.test/\"x", "https://a.test/\\x", "https://a.test/ x", "https://bücher.test", "not a url", "https://"] {
            assert!(signature_agent(value).unwrap_err().contains("--signature-agent"), "{value}");
        }
    }

    #[test]
    fn authority_drops_default_ports_and_lowercases_hosts() {
        assert_eq!(authority("https://Example.COM:443/x?y#z").unwrap(), "example.com");
        assert_eq!(authority("http://127.0.0.1:8080/").unwrap(), "127.0.0.1:8080");
        assert_eq!(authority("http://[::1]:80/").unwrap(), "[::1]");
        for url in ["/relative", "data:text/plain,x", "file:///tmp/x"] {
            assert!(authority(url).unwrap_err().contains("absolute http(s) URL"), "{url}");
        }
    }

    #[test]
    fn base64url_round_trips_and_refuses_the_standard_alphabet() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", &[0xfb, 0xff]] {
            assert_eq!(base64url_decode(&base64url_encode(bytes)).unwrap(), bytes);
        }
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
        for text in ["+/8", "-_8=", "A", "a b"] {
            assert!(base64url_decode(text).is_err(), "{text}");
        }
    }
}
