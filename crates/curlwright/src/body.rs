use crate::parse::{DataKind, DataSpec, FormSpec, Options, Stop};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ResolvedBody {
    pub bytes: Option<Vec<u8>>,
    pub form: Option<Vec<FormPart>>,
    pub content_type: Option<String>,
    pub accept: Option<String>,
}

#[derive(Clone, Debug)]
pub enum FormPart {
    Text { name: String, value: String },
    File { name: String, filename: String, mime: String, bytes: Vec<u8> },
}

enum StdinState {
    Unused,
    Consumed,
}

fn take_stdin(state: &mut StdinState) -> Result<Vec<u8>, Stop> {
    match std::mem::replace(state, StdinState::Consumed) {
        StdinState::Unused => read_all_stdin(),
        StdinState::Consumed => Err(Stop::usage(
            "curlwright: stdin (@-) can only be used once per command",
        )),
    }
}

fn read_all_stdin() -> Result<Vec<u8>, Stop> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buf)
        .map_err(|err| Stop::new(format!("curlwright: cannot read stdin: {err}"), 26))?;
    Ok(buf)
}

pub fn resolve_path(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
    }
}

fn read_bytes(path: &str) -> Result<Vec<u8>, Stop> {
    match std::fs::read(resolve_path(path)) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(Stop::new(format!("curlwright: cannot read {path}"), 26)),
    }
}

fn strip_newlines(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|byte| *byte != b'\n' && *byte != b'\r').collect()
}

fn encode_uri_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn resolve_url_encoded(value: &str, stdin: &mut StdinState) -> Result<Vec<u8>, Stop> {
    let eq = value.find('=');
    let at = value.find('@');
    let use_file = at.is_some() && eq.map(|eq| at.unwrap() < eq).unwrap_or(true);
    let split_at = if use_file { at } else { eq };
    let name = match split_at {
        Some(index) => &value[..index],
        None => "",
    };
    let rest = match split_at {
        Some(index) => &value[index + 1..],
        None => value,
    };
    let content = if use_file {
        let bytes = if rest == "-" {
            take_stdin(stdin)?
        } else {
            read_bytes(rest)?
        };
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        rest.to_string()
    };
    let encoded = encode_uri_component(&content);
    let text = if name.is_empty() { encoded } else { format!("{name}={encoded}") };
    Ok(text.into_bytes())
}

fn resolve_data_spec(spec: &DataSpec, stdin: &mut StdinState) -> Result<Vec<u8>, Stop> {
    if spec.kind == DataKind::UrlEncode {
        return resolve_url_encoded(&spec.value, stdin);
    }
    if spec.kind == DataKind::Raw || !spec.value.starts_with('@') {
        return Ok(spec.value.as_bytes().to_vec());
    }
    let path = &spec.value[1..];
    let bytes = if path == "-" {
        take_stdin(stdin)?
    } else {
        read_bytes(path)?
    };
    if spec.kind == DataKind::Binary {
        Ok(bytes)
    } else {
        Ok(strip_newlines(&bytes))
    }
}

fn join_parts(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push(b'&');
        }
        out.extend_from_slice(part);
    }
    out
}

fn form_option(options: &[&str], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    for option in options {
        let trimmed = option.trim();
        if let Some(value) = trimmed.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

fn basename(path: &str) -> String {
    let cleaned = path.trim_end_matches('/');
    cleaned.rsplit('/').next().unwrap_or(cleaned).to_string()
}

fn append_form_part(form: &mut Vec<FormPart>, spec: &FormSpec, stdin: &mut StdinState) -> Result<(), Stop> {
    let Some(eq) = spec.value.find('=') else {
        return Err(Stop::usage(format!(
            "curlwright: -F requires name=content (got \"{}\")",
            spec.value
        )));
    };
    let name = spec.value[..eq].to_string();
    let rest = &spec.value[eq + 1..];
    if spec.literal || (!rest.starts_with('@') && !rest.starts_with('<')) {
        form.push(FormPart::Text { name, value: rest.to_string() });
        return Ok(());
    }
    let raw = &rest[1..];
    let mut pieces = raw.split(';');
    let path = pieces.next().unwrap_or("");
    let options: Vec<&str> = pieces.collect();
    let bytes = if path == "-" {
        take_stdin(stdin)?
    } else {
        read_bytes(path)?
    };
    if rest.starts_with('<') {
        form.push(FormPart::Text {
            name,
            value: String::from_utf8_lossy(&bytes).into_owned(),
        });
        return Ok(());
    }
    let mime = form_option(&options, "type").unwrap_or_else(|| "application/octet-stream".to_string());
    let filename = if path == "-" {
        form_option(&options, "filename").unwrap_or_else(|| "-".to_string())
    } else {
        form_option(&options, "filename").unwrap_or_else(|| basename(path))
    };
    form.push(FormPart::File { name, filename, mime, bytes });
    Ok(())
}

pub fn resolve_body(opts: &Options) -> Result<ResolvedBody, Stop> {
    if !opts.data.is_empty() && !opts.form.is_empty() {
        return Err(Stop::usage("curlwright: -F cannot be combined with -d"));
    }
    let mut stdin = StdinState::Unused;
    if !opts.form.is_empty() {
        let mut form = Vec::new();
        for spec in &opts.form {
            append_form_part(&mut form, spec, &mut stdin)?;
        }
        return Ok(ResolvedBody {
            bytes: None,
            form: Some(form),
            content_type: None,
            accept: None,
        });
    }
    if opts.data.is_empty() {
        return Ok(ResolvedBody {
            bytes: None,
            form: None,
            content_type: None,
            accept: None,
        });
    }
    let mut parts = Vec::new();
    for spec in &opts.data {
        parts.push(resolve_data_spec(spec, &mut stdin)?);
    }
    let json = opts.data.iter().any(|spec| spec.kind == DataKind::Json);
    Ok(ResolvedBody {
        bytes: Some(join_parts(&parts)),
        form: None,
        content_type: Some(if json {
            "application/json".to_string()
        } else {
            "application/x-www-form-urlencoded".to_string()
        }),
        accept: if json { Some("application/json".to_string()) } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(kind: DataKind, value: &str) -> DataSpec {
        DataSpec { kind, value: value.to_string() }
    }

    #[test]
    fn data_ascii_strips_newlines_from_file() {
        let dir = std::env::temp_dir().join(format!("cw-body-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("input.txt");
        std::fs::write(&file, b"line1\nline2\r\nline3").unwrap();
        let spec = data(DataKind::Ascii, &format!("@{}", file.display()));
        let mut stdin = StdinState::Unused;
        let bytes = resolve_data_spec(&spec, &mut stdin).unwrap();
        assert_eq!(bytes, b"line1line2line3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_binary_preserves_newlines_from_file() {
        let dir = std::env::temp_dir().join(format!("cw-bodybin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("input.bin");
        std::fs::write(&file, b"a\nb\r\nc").unwrap();
        let spec = data(DataKind::Binary, &format!("@{}", file.display()));
        let mut stdin = StdinState::Unused;
        let bytes = resolve_data_spec(&spec, &mut stdin).unwrap();
        assert_eq!(bytes, b"a\nb\r\nc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_raw_keeps_at_dash_literal() {
        let spec = data(DataKind::Raw, "@-");
        let mut stdin = StdinState::Unused;
        let bytes = resolve_data_spec(&spec, &mut stdin).unwrap();
        assert_eq!(bytes, b"@-");
    }

    #[test]
    fn stdin_consumed_twice_is_refused() {
        let mut stdin = StdinState::Consumed;
        let result = take_stdin(&mut stdin);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("only be used once"), "{}", err.message);
    }

    #[test]
    fn strip_newlines_removes_cr_and_lf() {
        assert_eq!(strip_newlines(b"a\r\nb\nc\rd"), b"abcd");
        assert_eq!(strip_newlines(b"no-newlines"), b"no-newlines");
    }

    #[test]
    fn form_stdin_filename_defaults_to_dash() {
        let spec = FormSpec { value: "up=@-".to_string(), literal: false };
        let mut form = Vec::new();
        let mut stdin = StdinState::Unused;
        std::mem::replace(&mut stdin, StdinState::Consumed);
        let result = append_form_part(&mut form, &spec, &mut stdin);
        assert!(result.is_err());
    }
}
