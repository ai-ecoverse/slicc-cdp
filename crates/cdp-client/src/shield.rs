use crate::protected::Protected;
use serde_json::Value;
use std::collections::BTreeSet;

const ORIGIN_KEYS: &[&str] = &[
    "origin",
    "securityOrigin",
    "ownerOrigin",
    "issuerOrigin",
    "topLevelOrigin",
    "topLevelSite",
    "embeddedOrigin",
    "requestingOrigin",
    "scopeURL",
    "manifestId",
];

const FREE_FORM: &[&str] = &[
    "expression",
    "arguments",
    "value",
    "headers",
    "postData",
    "functionDeclaration",
    "source",
    "text",
    "body",
    "data",
    "message",
];

const BODY_METHODS: &[&str] = &[
    "Network.getResponseBody",
    "Network.searchInResponseBody",
    "Network.streamResourceContent",
    "Network.getResponseBodyForInterception",
    "Network.takeResponseBodyForInterceptionAsStream",
    "Network.getRequestPostData",
    "Network.replayXHR",
    "Fetch.getResponseBody",
    "Fetch.takeResponseBodyAsStream",
    "Audits.getEncodedResponse",
];

const INTERCEPTION_METHODS: &[&str] = &[
    "Fetch.fulfillRequest",
    "Fetch.failRequest",
    "Fetch.continueRequest",
    "Fetch.continueWithAuth",
    "Fetch.continueResponse",
    "Fetch.getResponseBody",
    "Fetch.takeResponseBodyAsStream",
    "Network.continueInterceptedRequest",
    "Network.getResponseBodyForInterception",
    "Network.takeResponseBodyForInterceptionAsStream",
];

const COOKIE_SETTERS: &[&str] = &[
    "Network.setCookie",
    "Network.setCookies",
    "Network.deleteCookies",
    "Storage.setCookies",
    "Page.deleteCookie",
];

const COOKIE_READERS: &[&str] = &[
    "Network.getCookies",
    "Network.getAllCookies",
    "Storage.getCookies",
    "Page.getCookies",
];

pub fn browser_wide(
    method: &str,
    params: &Value,
    session_id: Option<&str>,
    contexts: &BTreeSet<String>,
) -> Option<String> {
    let own = params
        .get("browserContextId")
        .and_then(|v| v.as_str())
        .is_some_and(|id| contexts.contains(id));
    let reason = match method {
        "Target.exposeDevToolsProtocol" => {
            "Target.exposeDevToolsProtocol would give a page the whole browser"
        }
        "Target.sendMessageToTarget" => {
            "Target.sendMessageToTarget is refused: attach with flatten: true instead"
        }
        "Target.attachToBrowserTarget" => {
            "Target.attachToBrowserTarget would give a page the whole browser"
        }
        "Tracing.start" => "Tracing.start records every target, seven's included",
        "Extensions.loadUnpacked" => "Extensions.loadUnpacked can reach seven's origin",
        "Network.clearBrowserCookies" => "Network.clearBrowserCookies would clear seven's cookies",
        "Storage.clearCookies" if !own => "Storage.clearCookies would clear seven's cookies",
        "Browser.resetPermissions" if !own => {
            "Browser.resetPermissions would reset seven's permissions"
        }
        "Browser.setPermission" | "Browser.grantPermissions"
            if !own && params.get("origin").and_then(|v| v.as_str()).is_none() =>
        {
            "browser-wide permissions would reach seven's origin"
        }
        "Network.setRequestInterception" => {
            "Network.setRequestInterception would hand out seven's requests; use Fetch on a page session"
        }
        _ if method.starts_with("Fetch.") && session_id.is_none() => {
            "Fetch on the browser itself would intercept seven's requests; use a page session"
        }
        _ => return None,
    };
    Some(format!("slicc: refused: {reason}\n"))
}

pub fn body_method(method: &str) -> bool {
    BODY_METHODS.contains(&method)
}

pub fn interception_method(method: &str) -> bool {
    INTERCEPTION_METHODS.contains(&method)
}

pub fn answers_interception(method: &str) -> bool {
    interception_method(method) && !method.contains("ResponseBody")
}

pub fn cookie_method(method: &str) -> bool {
    COOKIE_SETTERS.contains(&method)
}

pub fn cookie_reader(method: &str) -> bool {
    COOKIE_READERS.contains(&method)
}

pub fn request_key(params: &Value) -> Option<String> {
    params
        .get("requestId")
        .or_else(|| params.get("interceptionId"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn request_url(params: &Value) -> Option<String> {
    params
        .pointer("/request/url")
        .or_else(|| params.pointer("/response/url"))
        .or_else(|| params.get("url"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn web_socket_as_http(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("wss:") {
        format!("https:{}", &url[4..])
    } else if lower.starts_with("ws:") {
        format!("http:{}", &url[3..])
    } else {
        url.to_string()
    }
}

fn url_protected(guard: &Protected, text: &str) -> bool {
    guard.url(text) || guard.url(&web_socket_as_http(text))
}

pub fn mentions(guard: &Protected, value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains(':') && url_protected(guard, text),
        Value::Array(items) => items.iter().any(|item| mentions(guard, item)),
        Value::Object(map) => map.values().any(|item| mentions(guard, item)),
        _ => false,
    }
}

pub fn names_protected_storage(guard: &Protected, value: &Value) -> bool {
    match value {
        Value::Array(items) => items
            .iter()
            .any(|item| names_protected_storage(guard, item)),
        Value::Object(map) => map.iter().any(|(key, item)| {
            if FREE_FORM.contains(&key.as_str()) {
                return false;
            }
            if let Some(text) = item.as_str() {
                if ORIGIN_KEYS.contains(&key.as_str()) && url_protected(guard, text) {
                    return true;
                }
                if key == "storageKey" && guard.storage_key(text) {
                    return true;
                }
                if key == "cacheId" && guard.storage_key(text.split('|').next().unwrap_or("")) {
                    return true;
                }
                return false;
            }
            names_protected_storage(guard, item)
        }),
        _ => false,
    }
}

fn host_of(text: &str) -> Option<String> {
    url::Url::parse(text)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

fn site_protected(guard: &Protected, site: &str) -> bool {
    url_protected(guard, site) || host_of(site).is_some_and(|host| guard.cookie_domain(&host))
}

fn cookie_protected(guard: &Protected, cookie: &Value) -> bool {
    if let Some(domain) = cookie.get("domain").and_then(|v| v.as_str()) {
        if guard.cookie_domain(domain) {
            return true;
        }
    }
    if let Some(url) = cookie.get("url").and_then(|v| v.as_str()) {
        if url_protected(guard, url) || host_of(url).is_some_and(|host| guard.cookie_domain(&host))
        {
            return true;
        }
    }
    match cookie.get("partitionKey") {
        Some(Value::String(site)) => site_protected(guard, site),
        Some(key) => key
            .get("topLevelSite")
            .and_then(|v| v.as_str())
            .is_some_and(|site| site_protected(guard, site)),
        None => false,
    }
}

pub fn cookie_refused(guard: &Protected, params: &Value) -> bool {
    match params.get("cookies").and_then(|v| v.as_array()) {
        Some(cookies) => cookies.iter().any(|cookie| cookie_protected(guard, cookie)),
        None => cookie_protected(guard, params),
    }
}

pub fn drop_protected_cookies(guard: &Protected, result: &mut Value) {
    if let Some(cookies) = result.get_mut("cookies").and_then(|v| v.as_array_mut()) {
        cookies.retain(|cookie| !cookie_protected(guard, cookie));
    }
}

pub fn strip_protected_associated(guard: &Protected, event: &mut Value) {
    if let Some(cookies) = event
        .pointer_mut("/params/associatedCookies")
        .and_then(|v| v.as_array_mut())
    {
        cookies.retain(|item| {
            !item
                .get("cookie")
                .is_some_and(|cookie| cookie_protected(guard, cookie))
        });
    }
}

pub fn redacted(info: &Value) -> Value {
    let mut out = info.clone();
    let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let origin = url::Url::parse(url)
        .ok()
        .map(|parsed| parsed.origin().ascii_serialization())
        .filter(|origin| origin != "null")
        .unwrap_or_else(|| "about:blank".to_string());
    out["url"] = Value::String(origin.clone());
    out["title"] = Value::String(origin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn origins_and_storage_keys_are_found_outside_free_form_fields() {
        let guard = Protected::default();
        assert!(names_protected_storage(
            &guard,
            &json!({ "origin": "https://seven.sliccy.ai" })
        ));
        assert!(names_protected_storage(
            &guard,
            &json!({ "securityOrigin": "https://vault.sliccy.ai" })
        ));
        assert!(names_protected_storage(
            &guard,
            &json!({ "storageKey": "https://seven.sliccy.ai/" })
        ));
        assert!(names_protected_storage(
            &guard,
            &json!({ "cacheId": "https://seven.sliccy.ai/|c1" })
        ));
        assert!(names_protected_storage(
            &guard,
            &json!({ "bucketFileSystemLocator": { "storageKey": "https://sliccy.ai/", "pathComponents": [] } })
        ));
        assert!(names_protected_storage(
            &guard,
            &json!({ "scopeURL": "https://seven.sliccy.ai/" })
        ));
        assert!(!names_protected_storage(
            &guard,
            &json!({ "origin": "https://example.com" })
        ));
        assert!(!names_protected_storage(
            &guard,
            &json!({ "expression": "'https://seven.sliccy.ai'" })
        ));
        assert!(!names_protected_storage(
            &guard,
            &json!({ "value": { "origin": "https://seven.sliccy.ai" } })
        ));
    }

    #[test]
    fn cookies_on_seven_are_refused_and_filtered() {
        let guard = Protected::default();
        for params in [
            json!({ "name": "x", "value": "1", "url": "https://seven.sliccy.ai/" }),
            json!({ "name": "x", "value": "1", "domain": ".sliccy.ai", "path": "/" }),
            json!({ "name": "x", "value": "1", "url": "http://seven.sliccy.ai:1/" }),
            json!({ "name": "x", "value": "1", "url": "https://example.com/", "partitionKey": { "topLevelSite": "https://sliccy.ai" } }),
            json!({ "cookies": [{ "name": "y", "url": "https://example.com/" }, { "name": "x", "domain": "vault.sliccy.ai" }] }),
        ] {
            assert!(cookie_refused(&guard, &params), "{params}");
        }
        assert!(!cookie_refused(
            &guard,
            &json!({ "name": "a", "url": "https://example.com/" })
        ));
        let mut result = json!({ "cookies": [
            { "name": "seven", "domain": "seven.sliccy.ai" },
            { "name": "apex", "domain": ".sliccy.ai" },
            { "name": "chips", "domain": "example.com", "partitionKey": { "topLevelSite": "https://sliccy.ai" } },
            { "name": "ok", "domain": "example.com" }
        ] });
        drop_protected_cookies(&guard, &mut result);
        assert_eq!(
            result,
            json!({ "cookies": [{ "name": "ok", "domain": "example.com" }] })
        );
    }

    #[test]
    fn browser_wide_commands_are_refused_unless_in_an_own_context() {
        let none = BTreeSet::new();
        for (method, params, session) in [
            (
                "Target.exposeDevToolsProtocol",
                json!({ "targetId": "T" }),
                None,
            ),
            ("Target.sendMessageToTarget", json!({}), None),
            ("Tracing.start", json!({}), None),
            ("Extensions.loadUnpacked", json!({ "path": "/x" }), None),
            (
                "Browser.grantPermissions",
                json!({ "permissions": ["geolocation"] }),
                None,
            ),
            ("Browser.resetPermissions", json!({}), None),
            ("Storage.clearCookies", json!({}), None),
            ("Network.clearBrowserCookies", json!({}), Some("S")),
            ("Fetch.enable", json!({}), None),
        ] {
            let refused = browser_wide(method, &params, session, &none).unwrap();
            assert!(refused.starts_with("slicc: refused: "), "{method}");
        }
        assert!(browser_wide("Fetch.enable", &json!({}), Some("S"), &none).is_none());
        assert!(browser_wide(
            "Browser.grantPermissions",
            &json!({ "origin": "https://example.com" }),
            None,
            &none
        )
        .is_none());
        let own: BTreeSet<String> = ["C1".to_string()].into();
        assert!(browser_wide(
            "Storage.clearCookies",
            &json!({ "browserContextId": "C1" }),
            None,
            &own
        )
        .is_none());
        assert!(browser_wide(
            "Browser.resetPermissions",
            &json!({ "browserContextId": "C1" }),
            None,
            &own
        )
        .is_none());
        assert!(browser_wide(
            "Browser.resetPermissions",
            &json!({ "browserContextId": "C2" }),
            None,
            &own
        )
        .is_some());
    }

    #[test]
    fn listing_keeps_only_the_origin() {
        let info = json!({ "targetId": "T", "type": "page", "url": "https://seven.sliccy.ai/os/?k=secret", "title": "SECRET" });
        let out = redacted(&info);
        assert_eq!(out["url"], "https://seven.sliccy.ai");
        assert_eq!(out["title"], "https://seven.sliccy.ai");
        assert_eq!(
            redacted(&json!({ "url": "about:blank", "title": "x" }))["url"],
            "about:blank"
        );
        assert_eq!(
            web_socket_as_http("WSS://seven.sliccy.ai/ws"),
            "https://seven.sliccy.ai/ws"
        );
        assert!(mentions(
            &Protected::default(),
            &json!({ "request": { "url": "wss://seven.sliccy.ai/ws" } })
        ));
        assert!(!mentions(
            &Protected::default(),
            &json!({ "url": "https://example.com/?next=https://seven.sliccy.ai/" })
        ));
    }

    use crate::cdp::{self as client, Cdp, COOKIES, STREAM, UNSEEN};
    use crate::fetch::{Decision, Fulfill, Interception, Pattern};
    use crate::raw::Raw;
    use std::time::{Duration, Instant};

    const REFUSED: &str = "slicc: refused: target is on seven's origin\n";

    fn live(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
    }

    fn refused(result: Result<Value, String>, what: &str) -> String {
        match result {
            Ok(value) => panic!("{what} was allowed: {value}"),
            Err(message) => {
                assert!(message.starts_with("slicc: refused: "), "{what}: {message}");
                message
            }
        }
    }

    fn until_blocked(cdp: &mut Cdp, session: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !cdp.session_blocked(session) {
            assert!(
                Instant::now() < deadline,
                "session {session} was never blocked"
            );
            let _ = cdp.pump(Instant::now() + Duration::from_millis(200));
        }
    }

    fn page_ids(raw: &mut Raw) -> std::collections::BTreeSet<String> {
        raw.call("Target.getTargets", json!({}), None).unwrap()["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|info| info["type"] == "page")
            .filter_map(|info| info["targetId"].as_str().map(str::to_string))
            .collect()
    }

    fn nothing_mentions(cdp: &Cdp, needle: &str) {
        for event in cdp.events() {
            let text = event.to_string();
            assert!(!text.contains(needle), "an event leaked {needle}: {text}");
        }
    }

    fn page_on(cdp: &mut Cdp, url: &str) -> (String, String) {
        let target = cdp
            .call("Target.createTarget", json!({ "url": url }), None)
            .unwrap()["targetId"]
            .as_str()
            .unwrap()
            .to_string();
        let session = client::attach(cdp, &target).unwrap();
        cdp.call("Page.enable", json!({}), Some(&session)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let href = cdp
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "document.readyState === 'complete' ? location.href : ''", "returnByValue": true }),
                    Some(&session),
                )
                .unwrap()
                .pointer("/result/value")
                .cloned()
                .unwrap_or(Value::Null);
            if href
                .as_str()
                .is_some_and(|href| href.starts_with(url.split('?').next().unwrap()))
            {
                return (target, session);
            }
            assert!(Instant::now() < deadline, "{url} never loaded: {href}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    #[ignore = "needs SLICC_FETCH_LIVE_WS from test/integration/fetch.test.mjs"]
    fn live_protected_targets() {
        let ws = live("SLICC_FETCH_LIVE_WS");
        let app = live("SLICC_FETCH_LIVE_APP");
        let seven = live("SLICC_FETCH_LIVE_SEVEN");
        let vault = live("SLICC_FETCH_LIVE_VAULT");
        let seven_origin = seven.trim_end_matches('/').to_string();
        let mut raw = Raw::open(&ws);
        let mut cdp = Cdp::connect(&ws).unwrap();

        let before = page_ids(&mut raw);
        for url in [
            format!("{seven}/"),
            format!("{seven}/nowhere"),
            "HTTP://SEVEN.SLICCY.AI/x".to_string(),
            format!("blob:{seven}/0b9e"),
            "https://seven.sliccy.ai/".to_string(),
            "https://www.sliccy.ai/".to_string(),
            "https://vault.sliccy.ai/".to_string(),
            format!("{vault}/"),
            "view-source:https://seven.sliccy.ai/".to_string(),
        ] {
            let message = refused(
                cdp.call("Target.createTarget", json!({ "url": url }), None),
                &url,
            );
            assert_eq!(message, REFUSED, "{url}");
        }
        let made: Vec<String> = page_ids(&mut raw).difference(&before).cloned().collect();
        assert!(made.is_empty(), "a refused createTarget made a page: {made:?}");
        let allowed = cdp
            .call(
                "Target.createTarget",
                json!({ "url": format!("{app}/?n=create") }),
                None,
            )
            .unwrap()["targetId"]
            .as_str()
            .unwrap()
            .to_string();
        cdp.call("Target.closeTarget", json!({ "targetId": allowed }), None)
            .unwrap();

        {
            let (target, session) = page_on(&mut cdp, "about:blank");
            let message = refused(
                cdp.call(
                    "Page.navigate",
                    json!({ "url": format!("{seven}/nowhere") }),
                    Some(&session),
                ),
                "navigate to seven",
            );
            assert_eq!(message, REFUSED);
            refused(
                cdp.call(
                    "Page.navigate",
                    json!({ "url": format!("view-source:{seven}/") }),
                    Some(&session),
                ),
                "navigate to view-source of seven",
            );
            let href = cdp
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "location.href", "returnByValue": true }),
                    Some(&session),
                )
                .unwrap()["result"]["value"]
                .clone();
            assert_eq!(href, "about:blank");
            client::detach(&mut cdp, &session);
            cdp.call("Target.closeTarget", json!({ "targetId": target }), None)
                .unwrap();
        }

        let seven_tab = raw.create(&format!("{seven}/landing?k=SECRETKEY"));
        let seven_session = raw.session(&seven_tab);
        raw.wait_href(&seven_session, &seven);
        let vault_tab = raw.create(&format!("{vault}/landing?k=SECRETKEY"));
        let vault_session = raw.session(&vault_tab);
        raw.wait_href(&vault_session, &vault);
        for (tab, origin) in [
            (&seven_tab, seven_origin.clone()),
            (&vault_tab, vault.trim_end_matches('/').to_string()),
        ] {
            let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
            let info = listed["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .find(|info| info["targetId"] == json!(tab))
                .cloned()
                .expect("the protected tab is still listed");
            assert_eq!(info["url"], json!(origin));
            assert_eq!(info["title"], json!(origin));
            assert!(cdp.is_protected_target(tab));
            for (method, params) in [
                (
                    "Target.attachToTarget",
                    json!({ "targetId": tab, "flatten": true }),
                ),
                ("Target.activateTarget", json!({ "targetId": tab })),
                ("Target.closeTarget", json!({ "targetId": tab })),
                ("Target.exposeDevToolsProtocol", json!({ "targetId": tab })),
                (
                    "Target.sendMessageToTarget",
                    json!({ "targetId": tab, "message": "{}" }),
                ),
                ("Browser.getWindowForTarget", json!({ "targetId": tab })),
            ] {
                refused(cdp.call(method, params, None), method);
            }
            let info = cdp
                .call("Target.getTargetInfo", json!({ "targetId": tab }), None)
                .unwrap();
            assert_eq!(info["targetInfo"]["url"], json!(origin));
            assert_eq!(info["targetInfo"]["title"], json!(origin));
        }

        raw.eval(
            &seven_session,
            "Boolean(window.open('about:blank', '_blank'))",
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let popup = loop {
            let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
            if let Some(found) = listed["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .find(|info| info["openerId"] == json!(seven_tab))
                .cloned()
            {
                break found;
            }
            assert!(Instant::now() < deadline, "no popup");
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(popup["url"], "about:blank");
        refused(
            cdp.call(
                "Target.attachToTarget",
                json!({ "targetId": popup["targetId"], "flatten": true }),
                None,
            ),
            "attach to seven's blank popup",
        );
        raw.close(popup["targetId"].as_str().unwrap());

        {
            let (target, session) = page_on(&mut cdp, &format!("{app}/?n=script"));
            cdp.call("Runtime.enable", json!({}), Some(&session))
                .unwrap();
            cdp.call(
                "Runtime.evaluate",
                json!({ "expression": format!("setTimeout(() => {{ location.href = {}; }}, 50); true", json!(format!("{seven}/nowhere?secret=SCRIPTSECRET"))) }),
                Some(&session),
            )
            .unwrap();
            until_blocked(&mut cdp, &session);
            refused(
                cdp.call(
                    "Runtime.evaluate",
                    json!({ "expression": "document.cookie" }),
                    Some(&session),
                ),
                "evaluate after a script navigation to seven",
            );
            refused(
                cdp.call(
                    "Target.attachToTarget",
                    json!({ "targetId": target, "flatten": true }),
                    None,
                ),
                "reattach after a script navigation to seven",
            );
            let info = cdp
                .call("Target.getTargetInfo", json!({ "targetId": target }), None)
                .unwrap();
            assert_eq!(info["targetInfo"]["url"], json!(seven_origin));
            assert_eq!(info["targetInfo"]["title"], json!(seven_origin));
            nothing_mentions(&cdp, "SCRIPTSECRET");
            raw.close(&target);
        }

        {
            let (target, session) = page_on(&mut cdp, "about:blank");
            let landing = format!("{vault}/landing?n=refresh");
            {
                let mut seen = Interception::on_session(
                    &mut cdp,
                    &target,
                    &session,
                    &[Pattern::request(format!("{app}/refresh*"))],
                )
                .unwrap();
                let _ = seen.notify(
                    "Page.navigate",
                    json!({ "url": format!("{app}/refresh") }),
                    Some(&session),
                );
                let paused = seen
                    .next(Duration::from_secs(10))
                    .unwrap()
                    .expect("the page paused");
                let body = format!("<!doctype html><meta http-equiv=\"refresh\" content=\"0;url={landing}\"><title>refresh</title>");
                seen.decide(&paused, &Decision::Fulfill(Fulfill::html(200, body)))
                    .unwrap();
                until_blocked(&mut seen, &session);
            }
            for (method, params) in [
                (
                    "Runtime.evaluate",
                    json!({ "expression": "document.cookie" }),
                ),
                ("Page.captureScreenshot", json!({})),
                (
                    "DOMSnapshot.captureSnapshot",
                    json!({ "computedStyles": [] }),
                ),
                (
                    "Input.dispatchKeyEvent",
                    json!({ "type": "keyDown", "key": "a" }),
                ),
                ("DOM.getDocument", json!({})),
                ("Network.getCookies", json!({})),
                ("Page.navigate", json!({ "url": format!("{app}/") })),
                ("Fetch.enable", json!({})),
            ] {
                refused(cdp.call(method, params, Some(&session)), method);
            }
            refused(
                cdp.call(
                    "Target.attachToTarget",
                    json!({ "targetId": target, "flatten": true }),
                    None,
                ),
                "reattach after the meta refresh",
            );
            let url = raw
                .call("Target.getTargetInfo", json!({ "targetId": target }), None)
                .unwrap()["targetInfo"]["url"]
                .as_str()
                .unwrap_or("")
                .to_string();
            assert!(url.starts_with(&vault), "the refresh left the tab on {url}");
            nothing_mentions(&cdp, "n=refresh");
            raw.close(&target);
        }

        {
            let history = raw.create(&format!("{seven}/nowhere?n=history"));
            let session = raw.session(&history);
            raw.wait_href(&session, &seven);
            raw.call(
                "Page.navigate",
                json!({ "url": format!("{app}/?n=history") }),
                Some(&session),
            )
            .unwrap();
            raw.wait_href(&session, &app);
            let _ = raw.call(
                "Target.detachFromTarget",
                json!({ "sessionId": session }),
                None,
            );
            let guarded = client::attach(&mut cdp, &history).unwrap();
            let entries = cdp
                .call("Page.getNavigationHistory", json!({}), Some(&guarded))
                .unwrap()["entries"]
                .as_array()
                .unwrap()
                .clone();
            let on_seven = entries
                .iter()
                .find(|entry| entry["url"].as_str().unwrap().starts_with(&seven))
                .unwrap();
            let plain = entries
                .iter()
                .find(|entry| {
                    !entry["url"].as_str().unwrap().starts_with(&seven)
                        && entry["url"] != "about:blank"
                })
                .unwrap();
            let message = refused(
                cdp.call(
                    "Page.navigateToHistoryEntry",
                    json!({ "entryId": on_seven["id"] }),
                    Some(&guarded),
                ),
                "history entry on seven",
            );
            assert_eq!(message, REFUSED);
            cdp.call(
                "Page.navigateToHistoryEntry",
                json!({ "entryId": plain["id"] }),
                Some(&guarded),
            )
            .unwrap();
            client::detach(&mut cdp, &guarded);
            raw.close(&history);
        }

        {
            raw.call(
                "Storage.setCookies",
                json!({ "cookies": [
                    { "name": "seven", "value": "COOKIESECRET", "domain": "seven.sliccy.ai", "path": "/" },
                    { "name": "chips", "value": "COOKIESECRET", "domain": "localhost", "path": "/", "secure": true, "partitionKey": { "topLevelSite": "https://sliccy.ai", "hasCrossSiteAncestor": false } }
                ] }),
                None,
            )
            .unwrap();
            let (target, session) = page_on(&mut cdp, "about:blank");
            let s = Some(session.as_str());
            cdp.call(
                "Network.setCookie",
                json!({ "name": "allowed", "value": "1", "url": format!("{app}/") }),
                s,
            )
            .unwrap();
            for (method, params, on_page) in [
                (
                    "Network.setCookie",
                    json!({ "name": "x", "value": "1", "url": format!("{seven}/") }),
                    true,
                ),
                (
                    "Network.setCookie",
                    json!({ "name": "x", "value": "1", "domain": "seven.sliccy.ai", "path": "/" }),
                    true,
                ),
                (
                    "Network.setCookie",
                    json!({ "name": "x", "value": "1", "url": "http://seven.sliccy.ai:1/" }),
                    true,
                ),
                (
                    "Network.setCookie",
                    json!({ "name": "x", "value": "1", "url": format!("{app}/"), "secure": true, "partitionKey": { "topLevelSite": "http://sliccy.ai", "hasCrossSiteAncestor": false } }),
                    true,
                ),
                (
                    "Network.setCookies",
                    json!({ "cookies": [{ "name": "y", "value": "1", "url": format!("{app}/") }, { "name": "x", "value": "1", "domain": "seven.sliccy.ai" }] }),
                    true,
                ),
                (
                    "Storage.setCookies",
                    json!({ "cookies": [{ "name": "x", "value": "1", "domain": "vault.sliccy.ai", "path": "/" }] }),
                    false,
                ),
                (
                    "Network.deleteCookies",
                    json!({ "name": "seven", "url": format!("{seven}/") }),
                    true,
                ),
            ] {
                let message = refused(
                    cdp.call(method, params, if on_page { s } else { None }),
                    method,
                );
                assert_eq!(message, COOKIES, "{method}");
            }
            refused(
                cdp.call("Network.clearBrowserCookies", json!({}), s),
                "clearBrowserCookies",
            );
            refused(
                cdp.call("Storage.clearCookies", json!({}), None),
                "Storage.clearCookies",
            );
            for (method, params, on_page) in [
                ("Storage.getCookies", json!({}), false),
                ("Network.getAllCookies", json!({}), true),
                (
                    "Network.getCookies",
                    json!({ "urls": [format!("{seven}/"), format!("{app}/")] }),
                    true,
                ),
            ] {
                let got = cdp
                    .call(method, params, if on_page { s } else { None })
                    .unwrap();
                let text = got.to_string();
                assert!(!text.contains("COOKIESECRET"), "{method}: {text}");
                assert!(text.contains("\"allowed\""), "{method}: {text}");
            }
            client::detach(&mut cdp, &session);
            cdp.call("Target.closeTarget", json!({ "targetId": target }), None)
                .unwrap();
        }

        {
            let (target, session) = page_on(&mut cdp, "about:blank");
            let s = Some(session.as_str());
            let perm = json!({ "name": "geolocation" });
            cdp.call(
                "Browser.setPermission",
                json!({ "permission": perm, "setting": "granted", "origin": app }),
                None,
            )
            .unwrap();
            for (method, params, on_page) in [
                (
                    "Browser.setPermission",
                    json!({ "permission": perm, "setting": "granted", "origin": seven_origin }),
                    false,
                ),
                (
                    "Browser.grantPermissions",
                    json!({ "permissions": ["geolocation"] }),
                    false,
                ),
                (
                    "Browser.grantPermissions",
                    json!({ "permissions": ["geolocation"], "origin": seven_origin }),
                    false,
                ),
                ("Browser.resetPermissions", json!({}), false),
                (
                    "Storage.clearDataForOrigin",
                    json!({ "origin": seven_origin, "storageTypes": "all" }),
                    false,
                ),
                (
                    "Storage.clearDataForStorageKey",
                    json!({ "storageKey": format!("{seven_origin}/"), "storageTypes": "all" }),
                    false,
                ),
                (
                    "Storage.getUsageAndQuota",
                    json!({ "origin": seven_origin }),
                    false,
                ),
                (
                    "CacheStorage.requestCacheNames",
                    json!({ "securityOrigin": seven_origin }),
                    true,
                ),
                (
                    "CacheStorage.requestEntries",
                    json!({ "cacheId": format!("{seven_origin}/|c1") }),
                    true,
                ),
                (
                    "IndexedDB.requestDatabaseNames",
                    json!({ "securityOrigin": seven_origin }),
                    true,
                ),
                (
                    "FileSystem.getDirectory",
                    json!({ "bucketFileSystemLocator": { "storageKey": format!("{seven_origin}/"), "pathComponents": [] } }),
                    true,
                ),
                (
                    "ServiceWorker.unregister",
                    json!({ "scopeURL": format!("{seven_origin}/") }),
                    true,
                ),
                (
                    "Target.exposeDevToolsProtocol",
                    json!({ "targetId": target }),
                    false,
                ),
                ("Tracing.start", json!({}), false),
                ("Fetch.enable", json!({}), false),
                (
                    "Network.loadNetworkResource",
                    json!({ "url": format!("{seven}/index.html"), "options": { "disableCache": true, "includeCredentials": true } }),
                    true,
                ),
                (
                    "Target.setAutoAttach",
                    json!({ "autoAttach": true, "flatten": true, "waitForDebuggerOnStart": false }),
                    false,
                ),
                ("IO.read", json!({ "handle": "not-ours" }), false),
            ] {
                refused(
                    cdp.call(method, params, if on_page { s } else { None }),
                    method,
                );
            }
            assert_eq!(
                cdp.call(
                    "Network.getResponseBody",
                    json!({ "requestId": "never-seen" }),
                    s
                )
                .unwrap_err(),
                UNSEEN
            );
            assert_eq!(
                cdp.call("IO.read", json!({ "handle": "not-ours" }), None)
                    .unwrap_err(),
                STREAM
            );
            cdp.call("Fetch.enable", json!({}), s).unwrap();
            cdp.call("Fetch.disable", json!({}), s).unwrap();
            client::detach(&mut cdp, &session);
            cdp.call("Target.closeTarget", json!({ "targetId": target }), None)
                .unwrap();
        }

        {
            let context = cdp
                .call("Target.createBrowserContext", json!({}), None)
                .unwrap()["browserContextId"]
                .as_str()
                .unwrap()
                .to_string();
            cdp.call(
                "Storage.setCookies",
                json!({ "cookies": [{ "name": "own", "value": "1", "domain": "example.test", "path": "/" }], "browserContextId": context }),
                None,
            )
            .unwrap();
            cdp.call(
                "Storage.clearCookies",
                json!({ "browserContextId": context }),
                None,
            )
            .unwrap();
            cdp.call(
                "Browser.resetPermissions",
                json!({ "browserContextId": context }),
                None,
            )
            .unwrap();
            cdp.call(
                "Target.disposeBrowserContext",
                json!({ "browserContextId": context }),
                None,
            )
            .unwrap();
        }

        {
            let (target, session) = page_on(&mut cdp, &format!("{app}/?n=fetch"));
            cdp.call("Network.enable", json!({}), Some(&session))
                .unwrap();
            let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
            let record = seen.clone();
            let forged = "<!doctype html><title>forged</title>";
            crate::fetch::enable(
                &mut cdp,
                &session,
                &[Pattern::request("*"), Pattern::response("*")],
            )
            .unwrap();
            cdp.set_decider(
                &session,
                Some(Box::new(move |paused: &crate::fetch::Paused| {
                    record.borrow_mut().push(paused.url.clone());
                    if paused.url.contains("sliccy") {
                        return Decision::Fulfill(Fulfill::html(200, forged));
                    }
                    Decision::default()
                })),
            );
            cdp.call(
                "Page.navigate",
                json!({ "url": format!("{app}/?n=next") }),
                Some(&session),
            )
            .unwrap();
            cdp.wait_load(&session).unwrap();
            cdp.call(
                "Runtime.evaluate",
                json!({ "expression": format!("setTimeout(() => {{ location.href = {}; }}, 50); true", json!(format!("{seven}/nowhere?n=away"))) }),
                Some(&session),
            )
            .unwrap();
            until_blocked(&mut cdp, &session);
            assert!(
                seen.borrow().iter().all(|url| !url.contains("sliccy")),
                "{:?}",
                seen.borrow()
            );
            assert!(seen.borrow().iter().any(|url| url.contains("n=next")));
            nothing_mentions(&cdp, "n=away");
            let url = raw
                .call("Target.getTargetInfo", json!({ "targetId": target }), None)
                .unwrap()["targetInfo"]["url"]
                .as_str()
                .unwrap_or("")
                .to_string();
            assert!(!url.contains("forged"));
            let probe = raw.session(&target);
            let title = raw.eval(&probe, "document.title");
            assert_ne!(title, "forged");
            raw.close(&target);
        }

        let hits = |raw: &mut Raw, scratch: &str| -> u64 {
            let probe = raw.session(scratch);
            raw.eval(
                &probe,
                &format!(
                    "fetch({}).then((r) => r.text())",
                    json!(format!("{app}/hits?path=/api/leak"))
                ),
            )
            .as_str()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0)
        };
        let scratch = raw.create(&format!("{app}/one"));
        std::thread::sleep(Duration::from_millis(500));

        {
            let leak = format!("{app}/api/leak");
            let before = hits(&mut raw, &scratch);
            let (target, session) = page_on(&mut cdp, "about:blank");
            let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
            let record = seen.clone();
            let refresh = format!("{app}/refresh-leak");
            let landing = format!(
                "{vault}/landing?leak={}",
                url::form_urlencoded::byte_serialize(leak.as_bytes()).collect::<String>()
            );
            crate::fetch::enable(&mut cdp, &session, &[Pattern::request("*")]).unwrap();
            cdp.set_decider(
                &session,
                Some(Box::new(move |paused: &crate::fetch::Paused| {
                    record.borrow_mut().push(paused.url.clone());
                    if paused.url.contains("refresh-leak") {
                        let body = format!("<!doctype html><meta http-equiv=\"refresh\" content=\"0;url={landing}\">");
                        return Decision::Fulfill(Fulfill::html(200, body));
                    }
                    Decision::default()
                })),
            );
            let _ = cdp.notify("Page.navigate", json!({ "url": refresh }), Some(&session));
            until_blocked(&mut cdp, &session);
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let _ = cdp.pump(Instant::now() + Duration::from_millis(100));
            }
            assert!(seen.borrow().iter().any(|url| url.contains("refresh-leak")));
            assert!(
                seen.borrow()
                    .iter()
                    .all(|url| !url.contains("leak") || url.contains("refresh-leak")),
                "{:?}",
                seen.borrow()
            );
            assert_eq!(
                hits(&mut raw, &scratch),
                before,
                "the vault page ran and sent its request"
            );
            nothing_mentions(&cdp, "LEAKKEY");
            raw.close(&target);
        }

        {
            let idp = live("SLICC_FETCH_LIVE_IDP");
            let leak = format!("{app}/api/leak");
            let before = hits(&mut raw, &scratch);
            let mut slipped = Cdp::connect(&ws).unwrap();
            let (target, session) = page_on(&mut slipped, &format!("{idp}/landing"));
            let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
            let record = seen.clone();
            crate::fetch::enable(&mut slipped, &session, &[Pattern::request("*")]).unwrap();
            slipped.set_decider(
                &session,
                Some(Box::new(move |paused: &crate::fetch::Paused| {
                    record.borrow_mut().push(paused.url.clone());
                    Decision::default()
                })),
            );
            let origin = idp.trim_end_matches('/').to_string();
            slipped.protect_origins(&[origin.as_str()]);
            let page = raw.session(&target);
            raw.eval(&page, &format!("fetch({}, {{ headers: {{ Authorization: 'Bearer LEAKKEY' }} }}).catch(() => {{}}), true", json!(leak)));
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let _ = slipped.pump(Instant::now() + Duration::from_millis(100));
            }
            assert!(
                seen.borrow().iter().all(|url| !url.contains("api/leak")),
                "{:?}",
                seen.borrow()
            );
            assert_eq!(
                hits(&mut raw, &scratch),
                before + 1,
                "the request was not continued unchanged"
            );
            nothing_mentions(&slipped, "LEAKKEY");
            nothing_mentions(&slipped, "api/leak");
            raw.close(&target);
        }
        raw.close(&scratch);

        nothing_mentions(&cdp, "SECRETKEY");
        nothing_mentions(&cdp, "COOKIESECRET");
        raw.close(&seven_tab);
        raw.close(&vault_tab);
    }

    fn fixture_hits(app: &str, path: &str) -> String {
        crate::net::http_probe(&format!("{app}/hits?path={path}"), "text/plain")
            .map(|response| String::from_utf8_lossy(&response.body).into_owned())
            .unwrap_or_else(|error| format!("unknown ({})", error.trim()))
    }

    fn guard_failed(cdp: &mut Cdp, url: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if cdp
                .paused_trail
                .iter()
                .any(|(_, seen, reason)| seen.starts_with(url) && *reason == "guard session")
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            let _ = cdp.pump(Instant::now() + Duration::from_millis(100));
        }
    }

    fn until_history(
        raw: &mut Raw,
        session: &str,
        what: &str,
        done: impl Fn(Option<&str>) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let current = match raw.call("Page.getNavigationHistory", json!({}), Some(session)) {
                Ok(history) => {
                    let index = history["currentIndex"].as_u64().unwrap_or(0) as usize;
                    Ok(history["entries"][index]["url"]
                        .as_str()
                        .unwrap_or("")
                        .to_string())
                }
                Err(error) if error.contains("Not attached to an active page") => Err(error),
                Err(error) => panic!("Page.getNavigationHistory failed: {error}"),
            };
            if done(current.as_deref().ok()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what} never happened: {current:?}"
            );
        }
    }

    #[test]
    #[ignore = "needs SLICC_FETCH_LIVE_WS from test/integration/fetch.test.mjs"]
    fn live_guard_armed_while_a_navigation_commits() {
        let ws = live("SLICC_FETCH_LIVE_WS");
        let app = live("SLICC_FETCH_LIVE_APP");
        let idp = live("SLICC_FETCH_LIVE_IDP");
        let seven = live("SLICC_FETCH_LIVE_SEVEN");
        let mut raw = Raw::open(&ws);
        let mut cdp = Cdp::connect(&ws).unwrap();
        let tab = raw.create("about:blank");
        let slow = raw.session(&tab);
        raw.call("Page.enable", json!({}), Some(&slow)).unwrap();
        raw.call(
            "Emulation.setCPUThrottlingRate",
            json!({ "rate": 200 }),
            Some(&slow),
        )
        .unwrap();
        let landing = format!("{app}/landing?n=window");
        let to: String = url::form_urlencoded::byte_serialize(landing.as_bytes()).collect();
        raw.call(
            "Page.navigate",
            json!({ "url": format!("{idp}/js-redirect?to={to}") }),
            Some(&slow),
        )
        .unwrap();
        until_history(&mut raw, &slow, "the js-redirect committed", |current| {
            current.is_some_and(|url| url.contains("/js-redirect"))
        });
        until_history(&mut raw, &slow, "the landing went pending", |current| {
            current.is_none()
        });
        let session = client::attach(&mut cdp, &tab).unwrap();
        raw.call(
            "Emulation.setCPUThrottlingRate",
            json!({ "rate": 1 }),
            Some(&slow),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let href = cdp
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "document.readyState === 'complete' ? location.href : ''", "returnByValue": true }),
                    Some(&session),
                )
                .map(|result| result["result"]["value"].clone());
            if href.as_ref().is_ok_and(|href| href == &json!(landing)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the landing never loaded: {href:?}"
            );
            let _ = cdp.pump(Instant::now() + Duration::from_millis(100));
        }

        let frame = format!("{seven}/code?n=frame");
        cdp.call(
            "Runtime.evaluate",
            json!({ "expression": format!("const f = document.createElement('iframe'); f.src = {}; document.body.appendChild(f); true", json!(frame)) }),
            Some(&session),
        )
        .unwrap();
        assert!(
            guard_failed(&mut cdp, &frame),
            "the guard never saw the seven iframe"
        );
        assert_eq!(
            fixture_hits(&app, "/code"),
            "0",
            "the seven iframe reached the network"
        );

        let main = format!("{seven}/dump?n=main");
        cdp.call(
            "Runtime.evaluate",
            json!({ "expression": format!("setTimeout(() => {{ location.href = {}; }}, 50); true", json!(main)) }),
            Some(&session),
        )
        .unwrap();
        assert!(
            guard_failed(&mut cdp, &main),
            "the guard never saw the seven navigation"
        );
        until_blocked(&mut cdp, &session);
        assert_eq!(
            fixture_hits(&app, "/dump"),
            "0",
            "the seven navigation reached the network"
        );
        nothing_mentions(&cdp, "n=main");
        nothing_mentions(&cdp, "n=frame");
        raw.close(&tab);
    }
}
