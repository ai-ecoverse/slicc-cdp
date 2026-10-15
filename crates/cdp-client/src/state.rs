pub fn wasm_config_root(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    for name in ["XDG_STATE_HOME", "XDG_CONFIG_HOME"] {
        if let Some(dir) = var(name).filter(|dir| dir.starts_with('/')) {
            return Some(dir);
        }
    }
    let home = var("HOME").filter(|home| home.starts_with('/'))?;
    Some(format!("{}/.config", home.trim_end_matches('/')))
}

pub fn wasm_cwd(var: impl Fn(&str) -> Option<String>) -> Option<std::path::PathBuf> {
    match var("PWD").filter(|dir| dir.starts_with('/')) {
        Some(dir) => Some(dir.into()),
        None => std::env::current_dir().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::{wasm_config_root, wasm_cwd};

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn state_home_wins_then_config_home_then_home() {
        let all = [
            ("XDG_STATE_HOME", "/s"),
            ("XDG_CONFIG_HOME", "/c"),
            ("HOME", "/h"),
        ];
        assert_eq!(wasm_config_root(env(&all)).as_deref(), Some("/s"));
        let config = [
            ("XDG_STATE_HOME", "rel"),
            ("XDG_CONFIG_HOME", "/c"),
            ("HOME", "/h"),
        ];
        assert_eq!(wasm_config_root(env(&config)).as_deref(), Some("/c"));
        let home = [("XDG_CONFIG_HOME", ""), ("HOME", "/home/cone/")];
        assert_eq!(
            wasm_config_root(env(&home)).as_deref(),
            Some("/home/cone/.config")
        );
        assert_eq!(
            wasm_config_root(env(&[("HOME", "/")])).as_deref(),
            Some("/.config")
        );
    }

    #[test]
    fn pwd_names_the_cwd_when_absolute() {
        let pwd = [("PWD", "/opt/readonly")];
        assert_eq!(wasm_cwd(env(&pwd)), Some("/opt/readonly".into()));
        assert_eq!(
            wasm_cwd(env(&[("PWD", "rel")])),
            std::env::current_dir().ok()
        );
    }

    #[test]
    fn no_absolute_home_means_no_root() {
        assert_eq!(wasm_config_root(env(&[])), None);
        assert_eq!(wasm_config_root(env(&[("HOME", "home")])), None);
    }
}
