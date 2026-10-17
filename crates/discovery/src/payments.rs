use serde_json::{json, Map, Value};

use crate::standards::{parse_challenge_params, split_challenges};

pub const RAW_CAP: usize = 1024;
const HEADER_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct PaymentSignal {
    pub standard: String,
    pub resource: String,
    pub summary: Value,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PaymentClassification {
    pub signals: Vec<PaymentSignal>,
    pub read_body: bool,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn capped(raw: &str) -> String {
    let mut end = raw.len().min(RAW_CAP);
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    raw[..end].to_string()
}

fn invalid(standard: &str, resource: &str, reason: &str, raw: Option<&str>) -> PaymentSignal {
    let challenge = raw.map_or(Value::Null, |text| Value::String(capped(text)));
    PaymentSignal {
        standard: standard.into(),
        resource: resource.into(),
        summary: json!({ "status": 402, "challenge": challenge }),
        reason: Some(reason.into()),
    }
}

fn sextet(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim().trim_end_matches('=');
    if trimmed.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in trimmed.bytes() {
        acc = (acc << 6) | sextet(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn decode_json(text: &str) -> Option<Value> {
    let bytes = decode_base64(text)?;
    serde_json::from_slice(&bytes).ok()
}

fn text_of(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(Value::Number(number)) => Value::String(number.to_string()),
        _ => Value::Null,
    }
}

fn resource_of(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::String(url)) if !url.is_empty() => url.clone(),
        Some(Value::Object(map)) => map
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .unwrap_or(fallback)
            .to_string(),
        _ => fallback.to_string(),
    }
}

fn x402_offer(accept: &Value, version: u64) -> Value {
    let amount = if version == 1 {
        accept.get("maxAmountRequired")
    } else {
        accept.get("amount")
    };
    json!({
        "scheme": text_of(accept.get("scheme")),
        "network": text_of(accept.get("network")),
        "amount": text_of(amount),
        "asset": text_of(accept.get("asset")),
        "payTo": text_of(accept.get("payTo")),
        "expires": Value::Null,
        "maxTimeoutSeconds": accept.get("maxTimeoutSeconds").cloned().unwrap_or(Value::Null),
    })
}

fn x402_signal(payload: &Value, version: u64, url: &str, raw: &str) -> PaymentSignal {
    let declared = payload.get("x402Version").and_then(Value::as_u64);
    let accepts = payload.get("accepts").and_then(Value::as_array);
    let Some(accepts) = accepts.filter(|_| declared == Some(version)) else {
        return invalid("x402", url, "no x402 payload", Some(raw));
    };
    let first = accepts.first().and_then(|accept| accept.get("resource"));
    let resource = if version == 1 {
        resource_of(first, url)
    } else {
        resource_of(payload.get("resource"), url)
    };
    let offers: Vec<Value> = accepts.iter().map(|accept| x402_offer(accept, version)).collect();
    PaymentSignal {
        standard: "x402".into(),
        resource: resource.clone(),
        summary: json!({ "status": 402, "version": version, "resource": resource, "offers": offers }),
        reason: None,
    }
}

fn mpp_offer(params: &std::collections::BTreeMap<String, String>, request: &Value) -> Value {
    let mut offer = Map::new();
    for key in ["method", "intent", "realm"] {
        offer.insert(key.into(), params.get(key).map_or(Value::Null, |v| Value::String(v.clone())));
    }
    offer.insert("amount".into(), text_of(request.get("amount")));
    offer.insert("asset".into(), text_of(request.get("currency")));
    offer.insert(
        "expires".into(),
        params.get("expires").map_or(Value::Null, |v| Value::String(v.clone())),
    );
    Value::Object(offer)
}

fn mpp_signals(headers: &[(String, String)], url: &str) -> Vec<PaymentSignal> {
    let mut offers = Vec::new();
    let mut failure: Option<String> = None;
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("www-authenticate") {
            continue;
        }
        for challenge in split_challenges(&capped_header(value)) {
            let trimmed = challenge.trim();
            let Some((scheme, rest)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) else {
                continue;
            };
            if !scheme.eq_ignore_ascii_case("payment") {
                continue;
            }
            let params = parse_challenge_params(rest);
            match params.get("request").and_then(|request| decode_json(request)) {
                Some(request) => offers.push(mpp_offer(&params, &request)),
                None => failure = Some(trimmed.to_string()),
            }
        }
    }
    if offers.is_empty() {
        return failure
            .map(|raw| vec![invalid("mpp", url, "request is not base64url JSON", Some(&raw))])
            .unwrap_or_default();
    }
    vec![PaymentSignal {
        standard: "mpp".into(),
        resource: url.into(),
        summary: json!({ "status": 402, "resource": url, "offers": offers }),
        reason: None,
    }]
}

fn capped_header(value: &str) -> String {
    let mut end = value.len().min(HEADER_CAP);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|ct| {
        let ct = ct.to_ascii_lowercase();
        let media = ct.split(';').next().unwrap_or("").trim();
        media == "application/json" || media.ends_with("+json")
    })
}

pub fn classify_payment(headers: &[(String, String)], url: &str) -> PaymentClassification {
    let mut signals = Vec::new();
    let required = header(headers, "payment-required");
    if let Some(raw) = required {
        signals.push(match decode_json(raw) {
            Some(payload) => x402_signal(&payload, 2, url, raw),
            None => invalid("x402", url, "PAYMENT-REQUIRED is not base64 JSON", Some(raw)),
        });
    }
    signals.extend(mpp_signals(headers, url));
    let json_body = is_json(header(headers, "content-type"));
    let read_body = required.is_none() && json_body;
    if required.is_none() && !json_body {
        if let Some(raw) = header(headers, "x-payment") {
            signals.push(invalid("x402", url, "no x402 payload", Some(raw)));
        }
    }
    PaymentClassification { signals, read_body }
}

pub fn classify_x402_body(body: Option<&str>, headers: &[(String, String)], url: &str) -> Option<PaymentSignal> {
    let legacy = header(headers, "x-payment");
    let Some(body) = body else {
        return Some(invalid("x402", url, "body unavailable", legacy));
    };
    match serde_json::from_str::<Value>(body) {
        Ok(payload) if payload.get("x402Version").is_some() || payload.get("accepts").is_some() => {
            let signal = x402_signal(&payload, 1, url, legacy.unwrap_or(""));
            if signal.reason.is_some() {
                return Some(invalid("x402", url, "no x402 payload", legacy));
            }
            Some(signal)
        }
        _ => legacy.map(|raw| invalid("x402", url, "no x402 payload", Some(raw))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
            let count = chunk.len() + 1;
            for i in 0..4 {
                if i < count {
                    out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn decodes_padded_unpadded_and_url_safe_base64() {
        assert_eq!(decode_base64("aGk="), Some(b"hi".to_vec()));
        assert_eq!(decode_base64("aGk"), Some(b"hi".to_vec()));
        assert_eq!(decode_base64("-_8"), Some(vec![0xfb, 0xff]));
        assert_eq!(decode_base64("a"), None);
        assert_eq!(decode_base64("a*b="), None);
    }

    #[test]
    fn x402_v2_reads_the_header_and_keeps_amounts_as_sent() {
        let payload = r#"{"x402Version":2,"resource":{"url":"https://x.test/r"},"accepts":[{"scheme":"exact","network":"eip155:8453","amount":"123456789012345678901234567890","asset":"0xA","payTo":"0xB","maxTimeoutSeconds":60},{"scheme":"exact","network":"n","amount":5}]}"#;
        let c = classify_payment(&h(&[("PAYMENT-REQUIRED", &b64(payload.as_bytes()))]), "https://x.test/p");
        assert!(!c.read_body);
        let s = &c.signals[0];
        assert_eq!(s.standard, "x402");
        assert_eq!(s.resource, "https://x.test/r");
        assert_eq!(s.summary["version"], 2);
        assert_eq!(s.summary["offers"][0]["amount"], "123456789012345678901234567890");
        assert_eq!(s.summary["offers"][0]["payTo"], "0xB");
        assert_eq!(s.summary["offers"][0]["maxTimeoutSeconds"], 60);
        assert_eq!(s.summary["offers"][1]["amount"], "5");
        assert_eq!(s.summary["offers"][1]["asset"], Value::Null);
    }

    #[test]
    fn x402_v2_with_a_bad_or_foreign_header_is_invalid_with_the_raw_header_only() {
        let c = classify_payment(&h(&[("payment-required", "!!not base64!!")]), "u");
        assert_eq!(c.signals[0].reason.as_deref(), Some("PAYMENT-REQUIRED is not base64 JSON"));
        assert_eq!(c.signals[0].summary, json!({"status": 402, "challenge": "!!not base64!!"}));
        let other = b64(br#"{"x402Version":3,"accepts":[]}"#);
        let c = classify_payment(&h(&[("payment-required", &other)]), "u");
        assert_eq!(c.signals[0].reason.as_deref(), Some("no x402 payload"));
        let long = "a".repeat(5000);
        let c = classify_payment(&h(&[("payment-required", &format!("{long}é"))]), "u");
        assert_eq!(c.signals[0].summary["challenge"].as_str().unwrap().len(), RAW_CAP);
    }

    #[test]
    fn mpp_decodes_each_payment_challenge_and_skips_other_schemes() {
        let request = b64(br#"{"amount":"1000","currency":"usd"}"#).replace('=', "").replace('+', "-").replace('/', "_");
        let value = format!(r#"Bearer realm="x", Payment id="a1", realm="shop", method="tempo", intent="charge", request="{request}", expires="2026-01-01T00:00:00Z""#);
        let c = classify_payment(&h(&[("WWW-Authenticate", &value), ("www-authenticate", "Basic")]), "https://s.test/r");
        assert_eq!(c.signals.len(), 1);
        let s = &c.signals[0];
        assert_eq!(s.standard, "mpp");
        assert_eq!(s.resource, "https://s.test/r");
        assert_eq!(s.summary["offers"][0], json!({"method": "tempo", "intent": "charge", "realm": "shop", "amount": "1000", "asset": "usd", "expires": "2026-01-01T00:00:00Z"}));
        let bad = classify_payment(&h(&[("www-authenticate", r#"Payment id="b", request="@@""#)]), "u");
        assert_eq!(bad.signals[0].reason.as_deref(), Some("request is not base64url JSON"));
        assert!(classify_payment(&h(&[("www-authenticate", "Payment")]), "u").signals.is_empty());
    }

    #[test]
    fn one_response_can_carry_both_and_a_json_402_without_a_header_reads_its_body() {
        let v2 = b64(br#"{"x402Version":2,"accepts":[{"scheme":"exact","amount":"1"}]}"#);
        let mpp = format!(r#"Payment method="m", request="{}""#, b64(br#"{"amount":"2"}"#));
        let c = classify_payment(&h(&[("payment-required", &v2), ("www-authenticate", &mpp)]), "u");
        let standards: Vec<&str> = c.signals.iter().map(|s| s.standard.as_str()).collect();
        assert_eq!(standards, ["x402", "mpp"]);
        assert_eq!(c.signals[0].resource, "u");
        let body = classify_payment(&h(&[("Content-Type", "application/json; charset=utf-8")]), "u");
        assert!(body.read_body && body.signals.is_empty());
        assert!(classify_payment(&h(&[("content-type", "application/problem+json")]), "u").read_body);
        assert!(!classify_payment(&h(&[("content-type", "text/html")]), "u").read_body);
        assert!(!classify_payment(&[], "u").read_body);
    }

    #[test]
    fn a_legacy_x_payment_header_with_no_payload_is_invalid_and_keeps_its_raw_challenge() {
        let c = classify_payment(&h(&[("X-Payment", "lightning"), ("content-type", "text/html")]), "u");
        assert_eq!(c.signals[0].standard, "x402");
        assert_eq!(c.signals[0].reason.as_deref(), Some("no x402 payload"));
        assert_eq!(c.signals[0].summary["challenge"], "lightning");
        assert!(classify_payment(&h(&[("content-type", "text/html")]), "u").signals.is_empty());
    }

    #[test]
    fn x402_v1_reads_the_body_and_reports_what_it_cannot_read() {
        let body = r#"{"x402Version":1,"accepts":[{"scheme":"exact","network":"base","maxAmountRequired":"10000","asset":"0xU","payTo":"0xP","resource":"https://v1.test/r","maxTimeoutSeconds":30}]}"#;
        let s = classify_x402_body(Some(body), &[], "https://v1.test/p").unwrap();
        assert_eq!(s.resource, "https://v1.test/r");
        assert_eq!(s.summary["version"], 1);
        assert_eq!(s.summary["offers"][0]["amount"], "10000");
        let gone = classify_x402_body(None, &h(&[("x-payment", "x")]), "u").unwrap();
        assert_eq!(gone.reason.as_deref(), Some("body unavailable"));
        assert_eq!(gone.summary["challenge"], "x");
        let wrong = classify_x402_body(Some(r#"{"x402Version":1}"#), &[], "u").unwrap();
        assert_eq!(wrong.reason.as_deref(), Some("no x402 payload"));
        assert!(classify_x402_body(Some(r#"{"error":"x"}"#), &[], "u").is_none());
        assert!(classify_x402_body(Some("not json"), &[], "u").is_none());
        let legacy = classify_x402_body(Some("{}"), &h(&[("x-payment", "l")]), "u").unwrap();
        assert_eq!(legacy.reason.as_deref(), Some("no x402 payload"));
    }
}
