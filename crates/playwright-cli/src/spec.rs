#[derive(Clone, Copy)]
pub struct Spec {
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub variadic: bool,
    pub flags: &'static [(&'static str, bool)],
    pub implemented: bool,
    pub ref_mode: RefMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefMode {
    None,
    Element,
    Main,
}

pub fn lookup(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|spec| spec.name == name)
}

pub fn all() -> &'static [Spec] {
    SPECS
}

const S: bool = true;
const B: bool = false;

static SPECS: &[Spec] = &[
    spec("open", &["url"], false, &[("foreground", B), ("fg", B), ("runtime", S), ("discover", B), ("teleport-start", S), ("teleport-return", S), ("timeout", S), ("teleport-runtime", S)], true, RefMode::None),
    spec("tab-new", &["url"], false, &[("foreground", B), ("fg", B), ("runtime", S), ("discover", B), ("teleport-start", S), ("teleport-return", S), ("timeout", S), ("teleport-runtime", S)], true, RefMode::None),
    spec("close", &[], false, &[("tab", S)], true, RefMode::None),
    spec("goto", &["url"], false, &[("tab", S), ("discover", B), ("teleport-start", S), ("teleport-return", S), ("timeout", S), ("teleport-runtime", S)], true, RefMode::None),
    spec("type", &["text"], true, &[("tab", S), ("submit", B)], true, RefMode::None),
    spec("click", &["ref", "button"], false, &[("tab", S), ("modifiers", S)], true, RefMode::Element),
    spec("fill", &["ref", "text"], true, &[("tab", S), ("submit", B)], true, RefMode::Element),
    spec("press", &["key"], false, &[("tab", S)], true, RefMode::None),
    spec("snapshot", &[], false, &[("tab", S), ("frame", S), ("filename", S), ("no-iframes", B)], true, RefMode::None),
    spec("eval", &["expression"], true, &[("tab", S), ("frame", S), ("output", S), ("filename", S)], true, RefMode::None),
    spec("screenshot", &["ref"], false, &[("tab", S), ("filename", S), ("fullPage", B), ("full-page", B), ("max-width", S)], true, RefMode::Main),
    spec("tab-list", &[], false, &[], true, RefMode::None),
    spec("tab-select", &["index"], false, &[], true, RefMode::None),
    spec("tab-close", &[], false, &[("tab", S)], true, RefMode::None),
    spec("dblclick", &["ref", "button"], false, &[("tab", S), ("modifiers", S)], false, RefMode::Element),
    spec("drag", &["startRef", "endRef"], false, &[("tab", S)], false, RefMode::Element),
    spec("hover", &["ref"], false, &[("tab", S)], false, RefMode::Element),
    spec("select", &["ref", "val"], true, &[("tab", S)], false, RefMode::Element),
    spec("check", &["ref"], false, &[("tab", S)], false, RefMode::Element),
    spec("uncheck", &["ref"], false, &[("tab", S)], false, RefMode::Element),
    spec("dialog-accept", &["prompt"], true, &[("tab", S)], false, RefMode::None),
    spec("dialog-dismiss", &[], false, &[("tab", S)], false, RefMode::None),
    spec("resize", &["w", "h"], false, &[("tab", S)], false, RefMode::None),
    spec("go-back", &[], false, &[("tab", S)], false, RefMode::None),
    spec("go-forward", &[], false, &[("tab", S)], false, RefMode::None),
    spec("reload", &[], false, &[("tab", S)], false, RefMode::None),
    spec("keydown", &["key"], false, &[("tab", S)], false, RefMode::None),
    spec("keyup", &["key"], false, &[("tab", S)], false, RefMode::None),
    spec("pdf", &[], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("upload", &["ref", "file"], true, &[("tab", S)], false, RefMode::Element),
    spec("cookie-list", &[], false, &[("tab", S), ("domain", S), ("path", S)], false, RefMode::None),
    spec("cookie-get", &["name"], false, &[("tab", S)], false, RefMode::None),
    spec("cookie-set", &["name", "value"], false, &[("tab", S), ("domain", S), ("path", S), ("expires", S), ("httpOnly", B), ("secure", B), ("sameSite", S)], false, RefMode::None),
    spec("cookie-delete", &["name"], false, &[("tab", S), ("domain", S), ("path", S)], false, RefMode::None),
    spec("cookie-clear", &[], false, &[("tab", S)], false, RefMode::None),
    spec("localstorage-list", &[], false, &[("tab", S)], false, RefMode::None),
    spec("localstorage-get", &["key"], false, &[("tab", S)], false, RefMode::None),
    spec("localstorage-set", &["key", "value"], true, &[("tab", S)], false, RefMode::None),
    spec("localstorage-delete", &["key"], false, &[("tab", S)], false, RefMode::None),
    spec("localstorage-clear", &[], false, &[("tab", S)], false, RefMode::None),
    spec("sessionstorage-list", &[], false, &[("tab", S)], false, RefMode::None),
    spec("sessionstorage-get", &["key"], false, &[("tab", S)], false, RefMode::None),
    spec("sessionstorage-set", &["key", "value"], true, &[("tab", S)], false, RefMode::None),
    spec("sessionstorage-delete", &["key"], false, &[("tab", S)], false, RefMode::None),
    spec("sessionstorage-clear", &[], false, &[("tab", S)], false, RefMode::None),
    spec("state-save", &["filename"], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("state-load", &["filename"], false, &[("tab", S)], false, RefMode::None),
    spec("network-state-set", &["state"], false, &[("tab", S)], false, RefMode::None),
    spec("console", &["min-level"], false, &[("tab", S), ("clear", B)], false, RefMode::None),
    spec("requests", &[], false, &[("tab", S), ("static", B), ("filter", S), ("clear", B)], false, RefMode::None),
    spec("request", &["index"], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("request-headers", &["index"], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("request-body", &["index"], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("response-headers", &["index"], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("response-body", &["index"], false, &[("tab", S), ("filename", S)], false, RefMode::None),
    spec("mousemove", &["x", "y"], false, &[("tab", S)], false, RefMode::None),
    spec("mousedown", &["button"], false, &[("tab", S)], false, RefMode::None),
    spec("mouseup", &["button"], false, &[("tab", S)], false, RefMode::None),
    spec("mousewheel", &["dx", "dy"], false, &[("tab", S)], false, RefMode::None),
    spec("drop", &["target"], false, &[("tab", S), ("path", S), ("data", S)], false, RefMode::None),
    spec("route", &["pattern"], false, &[("tab", S), ("status", S), ("body", S), ("content-type", S), ("header", S)], false, RefMode::None),
    spec("route-list", &[], false, &[("tab", S)], false, RefMode::None),
    spec("unroute", &["pattern"], false, &[("tab", S)], false, RefMode::None),
    spec("generate-locator", &["target"], false, &[("tab", S)], false, RefMode::None),
    spec("highlight", &["target"], false, &[("tab", S), ("hide", B), ("style", S)], false, RefMode::None),
    spec("eval-file", &["path"], false, &[("tab", S), ("frame", S), ("output", S)], false, RefMode::None),
    spec("navigate", &["url"], false, &[("tab", S), ("discover", B), ("teleport-start", S), ("teleport-return", S), ("timeout", S), ("teleport-runtime", S)], false, RefMode::None),
    spec("teleport", &[], false, &[("tab", S), ("start", S), ("return", S), ("teleport-start", S), ("teleport-return", S), ("timeout", S), ("runtime", S), ("off", B), ("list", B)], false, RefMode::None),
    spec("fetch", &["url"], false, &[("tab", S), ("method", S), ("discover", B)], false, RefMode::None),
    spec("frames", &[], false, &[("tab", S)], false, RefMode::None),
    spec("record", &["url"], false, &[("tab", S), ("filter", S)], false, RefMode::None),
    spec("stop-recording", &["recordingId"], false, &[("tab", S)], false, RefMode::None),
];

const fn spec(
    name: &'static str,
    args: &'static [&'static str],
    variadic: bool,
    flags: &'static [(&'static str, bool)],
    implemented: bool,
    ref_mode: RefMode,
) -> Spec {
    Spec {
        name,
        args,
        variadic,
        flags,
        implemented,
        ref_mode,
    }
}

pub fn flag_is_bool(spec: &Spec, name: &str) -> Option<bool> {
    spec.flags
        .iter()
        .find(|(flag, _)| *flag == name)
        .map(|(_, is_string)| !is_string)
}

pub fn ref_error(command: &str, spec: &Spec, positional: &[String]) -> Option<String> {
    if spec.variadic || spec.ref_mode == RefMode::None {
        return None;
    }
    let pattern_ok = |value: &str| match spec.ref_mode {
        RefMode::Main => is_main_ref(value),
        RefMode::Element => is_element_ref(value),
        RefMode::None => true,
    };
    for (index, name) in spec.args.iter().enumerate() {
        if !matches!(*name, "ref" | "startRef" | "endRef") {
            continue;
        }
        let Some(value) = positional.get(index) else {
            continue;
        };
        if pattern_ok(value) {
            continue;
        }
        let hint = if spec.flags.iter().any(|(flag, _)| *flag == "filename")
            && (value.contains('/') || value.contains('.'))
        {
            format!(" Use --filename={value} to choose where the output is saved.")
        } else {
            String::new()
        };
        let expected = if spec.ref_mode == RefMode::Main {
            "expected a main-frame ref like e5; take a snapshot without --frame"
        } else {
            "expected e5 or f1e5"
        };
        return Some(format!(
            "playwright-cli {command}: \"{value}\" is not an element ref ({expected}).{hint}\nRun \"playwright-cli {command} --help\" for usage.\n"
        ));
    }
    None
}

pub fn is_element_ref(value: &str) -> bool {
    let rest = value
        .strip_prefix('f')
        .and_then(|text| {
            let digits = text.chars().take_while(|ch| ch.is_ascii_digit()).count();
            if digits == 0 {
                None
            } else {
                Some(&text[digits..])
            }
        })
        .unwrap_or(value);
    let Some(rest) = rest.strip_prefix('e') else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit())
}

fn is_main_ref(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('e') else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) && !value.contains('f')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_refs() {
        assert!(is_element_ref("e1"));
        assert!(is_element_ref("e12"));
        assert!(is_element_ref("f1e5"));
        assert!(is_element_ref("f12e3"));
        assert!(!is_element_ref("e"));
        assert!(!is_element_ref("button"));
        assert!(!is_element_ref("fe5"));
    }

    #[test]
    fn screenshot_path_is_not_a_ref() {
        let spec = lookup("screenshot").unwrap();
        let err = ref_error("screenshot", spec, &["/tmp/shot.png".into()]).unwrap();
        assert!(err.contains("not an element ref"));
        assert!(err.contains("--filename=/tmp/shot.png"));
    }
}
