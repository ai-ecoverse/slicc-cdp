#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

pub(crate) fn attached_port() -> Result<Option<String>, String> {
    #[cfg(any(test, target_arch = "wasm32"))]
    {
        Ok(None)
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        let ports = cdp_client::ports::catalog();
        if let Some(url) = cdp_client::discover::attached(&mut |url| http_body(url))? {
            return Ok(Some(url));
        }
        Ok(cdp_client::ports::first_live(&ports, &mut |url| http_body(url)))
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
    fn tests_do_not_scan_the_disk() {
        assert_eq!(attached_port().unwrap(), None);
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
