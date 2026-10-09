use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct WriteOut {
    pub url_effective: String,
    pub http_code: i64,
    pub content_type: String,
    pub size_download: usize,
    pub size_header: usize,
    pub size_upload: usize,
    pub method: String,
    pub num_redirects: i64,
    pub time_total_seconds: f64,
    pub exit_code: i32,
    pub error_msg: String,
    pub response_headers: Vec<(String, String)>,
}

fn seconds_format(seconds: f64) -> String {
    format!("{seconds:.6}")
}

fn write_out_json(ctx: &WriteOut) -> String {
    let mut map = serde_json::Map::new();
    map.insert("url_effective".into(), json!(ctx.url_effective));
    map.insert("http_code".into(), json!(ctx.http_code));
    map.insert("content_type".into(), json!(ctx.content_type));
    map.insert("size_download".into(), json!(ctx.size_download));
    map.insert("size_header".into(), json!(ctx.size_header));
    map.insert("size_upload".into(), json!(ctx.size_upload));
    map.insert("method".into(), json!(ctx.method));
    map.insert("num_redirects".into(), json!(ctx.num_redirects));
    map.insert("time_total".into(), json!(ctx.time_total_seconds));
    map.insert("exitcode".into(), json!(ctx.exit_code));
    map.insert("errormsg".into(), json!(ctx.error_msg));
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

fn lookup_variable(name: &str, ctx: &WriteOut) -> Option<String> {
    Some(match name {
        "url" | "url_effective" => ctx.url_effective.clone(),
        "http_code" | "response_code" => ctx.http_code.to_string(),
        "content_type" => ctx.content_type.clone(),
        "size_download" => ctx.size_download.to_string(),
        "size_header" => ctx.size_header.to_string(),
        "size_upload" | "size_request" => ctx.size_upload.to_string(),
        "method" => ctx.method.clone(),
        "num_redirects" => ctx.num_redirects.to_string(),
        "time_total" | "time_starttransfer" => seconds_format(ctx.time_total_seconds),
        "exitcode" => ctx.exit_code.to_string(),
        "errormsg" => ctx.error_msg.clone(),
        "json" => write_out_json(ctx),
        _ => return None,
    })
}

fn header_value(name: &str, ctx: &WriteOut) -> String {
    ctx.response_headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

fn read_braced(chars: &[char], open: usize) -> Option<(String, usize)> {
    let close = chars[open + 1..].iter().position(|ch| *ch == '}')?;
    let body: String = chars[open + 1..open + 1 + close].iter().collect();
    Some((body, open + 1 + close))
}

fn starts_at(chars: &[char], index: usize, needle: &str) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    if index + needle.len() > chars.len() {
        return false;
    }
    chars[index..index + needle.len()] == needle[..]
}

pub fn format_write_out(format: &str, ctx: &WriteOut) -> (String, Vec<String>) {
    let chars: Vec<char> = format.chars().collect();
    let mut warnings = Vec::new();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' && index + 1 < chars.len() {
            index += 1;
            out.push(match chars[index] {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            index += 1;
            continue;
        }
        if ch != '%' {
            out.push(ch);
            index += 1;
            continue;
        }
        if chars.get(index + 1) == Some(&'%') {
            out.push('%');
            index += 2;
            continue;
        }
        if chars.get(index + 1) == Some(&'{') {
            let Some((body, end)) = read_braced(&chars, index + 1) else {
                out.push(ch);
                index += 1;
                continue;
            };
            match lookup_variable(&body, ctx) {
                Some(value) => out.push_str(&value),
                None => {
                    warnings.push(format!("curlwright: unknown --write-out variable: '{body}'"));
                }
            }
            index = end + 1;
            continue;
        }
        if starts_at(&chars, index, "%header{") {
            if let Some((body, end)) = read_braced(&chars, index + 7) {
                out.push_str(&header_value(&body, ctx));
                index = end + 1;
                continue;
            }
        }
        out.push(ch);
        index += 1;
    }
    (out, warnings)
}
