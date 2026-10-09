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

fn resolve_url_encoded(value: &str) -> Result<Vec<u8>, Stop> {
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
        String::from_utf8_lossy(&read_bytes(rest)?).into_owned()
    } else {
        rest.to_string()
    };
    let encoded = encode_uri_component(&content);
    let text = if name.is_empty() { encoded } else { format!("{name}={encoded}") };
    Ok(text.into_bytes())
}

fn resolve_data_spec(spec: &DataSpec) -> Result<Vec<u8>, Stop> {
    if spec.kind == DataKind::UrlEncode {
        return resolve_url_encoded(&spec.value);
    }
    if spec.kind == DataKind::Raw || !spec.value.starts_with('@') {
        return Ok(spec.value.as_bytes().to_vec());
    }
    let bytes = read_bytes(&spec.value[1..])?;
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

fn append_form_part(form: &mut Vec<FormPart>, spec: &FormSpec) -> Result<(), Stop> {
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
    let bytes = read_bytes(path)?;
    if rest.starts_with('<') {
        form.push(FormPart::Text {
            name,
            value: String::from_utf8_lossy(&bytes).into_owned(),
        });
        return Ok(());
    }
    let mime = form_option(&options, "type").unwrap_or_else(|| "application/octet-stream".to_string());
    let filename = form_option(&options, "filename").unwrap_or_else(|| basename(path));
    form.push(FormPart::File { name, filename, mime, bytes });
    Ok(())
}

pub fn resolve_body(opts: &Options) -> Result<ResolvedBody, Stop> {
    if !opts.data.is_empty() && !opts.form.is_empty() {
        return Err(Stop::usage("curlwright: -F cannot be combined with -d"));
    }
    if !opts.form.is_empty() {
        let mut form = Vec::new();
        for spec in &opts.form {
            append_form_part(&mut form, spec)?;
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
        parts.push(resolve_data_spec(spec)?);
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
