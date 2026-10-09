fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../package.json");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let version = version_field(&text).unwrap_or_else(|| "0.0.0".to_string());
    println!("cargo:rustc-env=PLAYWRIGHT_CLI_VERSION={version}");
    println!("cargo:rerun-if-changed=../../package.json");
}

fn version_field(text: &str) -> Option<String> {
    let key = "\"version\"";
    let idx = text.find(key)?;
    let rest = &text[idx + key.len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')?;
    Some(rest[start..start + end].to_string())
}
