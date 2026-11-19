#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use cdp_client::resolve::{Finder, Rule, Step};

pub(crate) struct HostFinder;

impl Finder for HostFinder {
    fn attached(&mut self, trail: &mut Vec<Step>) -> Result<Option<(Rule, String)>, String> {
        #[cfg(any(test, target_arch = "wasm32"))]
        {
            let _ = trail;
            Ok(None)
        }
        #[cfg(all(not(test), not(target_arch = "wasm32")))]
        {
            cdp_client::discover::attached_traced(&mut |url| http_body(url), trail)
        }
    }

    fn saved(&mut self, trail: &mut Vec<Step>) -> Option<String> {
        #[cfg(any(test, target_arch = "wasm32"))]
        {
            let _ = trail;
            None
        }
        #[cfg(all(not(test), not(target_arch = "wasm32")))]
        {
            let text = crate::tab::session_text();
            cdp_client::resolve::saved_from_json(text.as_deref(), &mut |url| http_body(url), trail)
        }
    }

    fn probe(&mut self, trail: &mut Vec<Step>) -> Option<String> {
        #[cfg(any(test, target_arch = "wasm32"))]
        {
            let _ = trail;
            None
        }
        #[cfg(all(not(test), not(target_arch = "wasm32")))]
        {
            let ports = cdp_client::ports::catalog();
            cdp_client::resolve::probe_ports(&ports, &mut |url| http_body(url), trail)
        }
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
pub(crate) fn user_config_dir() -> String {
    config_dir(&current_env())
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
struct HostEnv {
    platform: &'static str,
    home: String,
    app_data: Option<String>,
    xdg_config_home: Option<String>,
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn config_dir(env: &HostEnv) -> String {
    match env.platform {
        "macos" => join_parts(
            false,
            &[
                env.home.clone(),
                "Library".to_string(),
                "Application Support".to_string(),
            ],
        ),
        "windows" => {
            if let Some(app_data) = &env.app_data {
                app_data.clone()
            } else {
                join_parts(
                    true,
                    &[
                        env.home.clone(),
                        "AppData".to_string(),
                        "Roaming".to_string(),
                    ],
                )
            }
        }
        _ => {
            if let Some(xdg) = &env.xdg_config_home {
                if xdg.starts_with('/') {
                    return xdg.clone();
                }
            }
            join_parts(false, &[env.home.clone(), ".config".to_string()])
        }
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn join_parts(windows: bool, parts: &[String]) -> String {
    let sep = if windows { '\\' } else { '/' };
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        let piece = if windows {
            part.replace('/', "\\")
        } else {
            part.clone()
        };
        let piece = if index == 0 {
            piece.trim_end_matches(sep).to_string()
        } else {
            piece.trim_matches(sep).to_string()
        };
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with(sep) {
            out.push(sep);
        }
        out.push_str(&piece);
    }
    out
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn current_env() -> HostEnv {
    HostEnv {
        platform: host_platform(),
        home: home_dir(),
        app_data: env_var("APPDATA"),
        xdg_config_home: env_var("XDG_CONFIG_HOME"),
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn home_dir() -> String {
    if cfg!(windows) {
        if let Some(home) = env_var("USERPROFILE") {
            return home;
        }
    }
    if let Some(home) = env_var("HOME") {
        return home;
    }
    env_var("USERPROFILE").unwrap_or_default()
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn http_body(url: &str) -> Result<String, String> {
    let response = cdp_client::net::http_get(url)?;
    Ok(String::from_utf8_lossy(&response.body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_do_not_scan_the_disk_or_dial() {
        let mut trail = Vec::new();
        assert_eq!(HostFinder.attached(&mut trail).unwrap(), None);
        assert_eq!(HostFinder.saved(&mut trail), None);
        assert_eq!(HostFinder.probe(&mut trail), None);
    }

    #[test]
    fn shared_discover_roots_include_seven_legacy_and_six() {
        let darwin = cdp_client::discover::HostEnv {
            platform: "macos",
            home: "/Users/me".to_string(),
            app_data: None,
            local_app_data: None,
            xdg_config_home: None,
            xdg_state_home: None,
        };
        let roots = cdp_client::discover::slicc_profile_roots(&darwin);
        assert!(roots.iter().any(|root| root.ends_with("/slicc/seven-profiles")));
        assert!(roots.iter().any(|root| root.ends_with("/slicc/profiles")));
        assert!(roots.iter().any(|root| root.ends_with("/Slicc/profiles")));
    }
}
