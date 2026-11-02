//! Log formats that are tables in disguise: logfmt, web access logs, and JSON lines that carry a
//! prefix (`docker compose logs`, `kubectl logs --prefix`).
//!
//! Each parser looks at the first few lines before it reads the rest, so text that is not one of
//! these formats is turned away quickly.

use crate::table::{Format, Table, MIN_ROWS};
use serde_json::Value;
use std::collections::HashMap;

const MAX_COLS: usize = 150;

fn is_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-/".contains(c))
}

/// One logfmt line: `key=value key2="quoted value"`. `None` when the line is not logfmt.
fn logfmt_line(line: &str) -> Option<Vec<(String, String)>> {
    let chars: Vec<char> = line.trim().chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let start = i;
        while i < chars.len() && chars[i] != '=' && chars[i] != ' ' {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect();
        if i >= chars.len() || chars[i] != '=' || !is_key(&key) {
            return None;
        }
        i += 1;
        let mut val = String::new();
        if i < chars.len() && chars[i] == '"' {
            i += 1;
            let mut closed = false;
            while i < chars.len() {
                match chars[i] {
                    '\\' if i + 1 < chars.len() => {
                        val.push(match chars[i + 1] {
                            'n' => '\n',
                            't' => '\t',
                            c => c,
                        });
                        i += 2;
                    }
                    '"' => {
                        closed = true;
                        i += 1;
                        break;
                    }
                    c => {
                        val.push(c);
                        i += 1;
                    }
                }
            }
            if !closed {
                return None;
            }
            if i < chars.len() && chars[i] != ' ' {
                return None;
            }
        } else {
            while i < chars.len() && chars[i] != ' ' {
                val.push(chars[i]);
                i += 1;
            }
        }
        out.push((key, val));
    }
    Some(out)
}

/// Rows from keyed cells. Columns are the union of keys in first-seen order.
fn table_from_pairs(
    format: Format,
    lead: Option<&str>,
    rows: Vec<(String, Vec<(String, String)>)>,
    notes: Vec<String>,
) -> Option<Table> {
    let mut headers: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    if let Some(l) = lead {
        headers.push(l.to_string());
        index.insert(l.to_string(), 0);
    }
    for (_, pairs) in &rows {
        for (k, _) in pairs {
            if !index.contains_key(k) {
                if headers.len() >= MAX_COLS {
                    return None;
                }
                index.insert(k.clone(), headers.len());
                headers.push(k.clone());
            }
        }
    }
    if rows.len() < MIN_ROWS || headers.len() < 2 {
        return None;
    }
    let n = headers.len();
    let out = rows
        .into_iter()
        .map(|(lead_value, pairs)| {
            let mut row = vec![String::new(); n];
            if lead.is_some() {
                row[0] = lead_value;
            }
            for (k, v) in pairs {
                row[index[&k]] = v;
            }
            row
        })
        .collect();
    Some(Table {
        format,
        headers,
        rows: out,
        notes,
    })
}

/// `key=value` lines with at least three pairs on almost every line.
pub fn parse_logfmt(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < MIN_ROWS {
        return None;
    }
    let looks = |l: &str| logfmt_line(l).is_some_and(|p| p.len() >= 3);
    if !lines.iter().take(5).all(|l| looks(l)) {
        return None;
    }
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for l in &lines {
        match logfmt_line(l) {
            Some(p) if p.len() >= 3 => rows.push((String::new(), p)),
            _ => {
                if notes.len() < 5 {
                    notes.push(l.chars().take(200).collect());
                }
            }
        }
    }
    if (lines.len() - rows.len()) * 20 > lines.len() {
        return None;
    }
    table_from_pairs(Format::Logfmt, None, rows, notes)
}

/// Splits `"quoted text" rest` at the closing quote, honouring `\"`.
fn split_quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix('"')?;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some((&s[..i], s[i + 1..].strip_prefix(' ').unwrap_or(&s[i + 1..]))),
            _ => i += 1,
        }
    }
    None
}

/// One line of an Apache or nginx access log (common or combined format).
fn access_line(line: &str) -> Option<Vec<String>> {
    let (host, rest) = line.split_once(' ')?;
    let (_ident, rest) = rest.split_once(' ')?;
    let (user, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('[')?;
    let (time, rest) = rest.split_once("] ")?;
    if host.is_empty() || !time.contains('/') || !time.contains(':') {
        return None;
    }
    let (request, rest) = split_quoted(rest)?;
    let (status, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    if status.len() != 3 || !status.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (bytes, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    if bytes != "-" && !(bytes.bytes().all(|c| c.is_ascii_digit()) && !bytes.is_empty()) {
        return None;
    }
    let (referer, agent) = match split_quoted(rest) {
        Some((r, tail)) => match split_quoted(tail) {
            Some((a, _)) => (r.to_string(), a.to_string()),
            None => return None,
        },
        None if rest.is_empty() => (String::new(), String::new()),
        None => return None,
    };
    let mut parts = request.splitn(3, ' ');
    let (method, path, proto) = (
        parts.next()?,
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    Some(vec![
        host.to_string(),
        user.to_string(),
        time.to_string(),
        method.to_string(),
        path.to_string(),
        proto.to_string(),
        status.to_string(),
        bytes.to_string(),
        referer,
        agent,
    ])
}

pub fn parse_access_log(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < MIN_ROWS || !lines.iter().take(5).all(|l| access_line(l).is_some()) {
        return None;
    }
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for l in &lines {
        match access_line(l) {
            Some(r) => rows.push(r),
            None => {
                if notes.len() < 5 {
                    notes.push(l.chars().take(200).collect());
                }
            }
        }
    }
    if (lines.len() - rows.len()) * 20 > lines.len() || rows.len() < MIN_ROWS {
        return None;
    }
    // "-" means "no value" in an access log.
    for r in &mut rows {
        for i in [1, 7, 8] {
            if r[i] == "-" {
                r[i].clear();
            }
        }
    }
    let mut headers: Vec<String> = [
        "ip",
        "user",
        "time",
        "method",
        "path",
        "protocol",
        "status",
        "bytes",
        "referer",
        "user_agent",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // The common format has no referer and no user agent: drop the two empty columns.
    if rows.iter().all(|r| r[8].is_empty() && r[9].is_empty()) {
        headers.truncate(8);
        for r in &mut rows {
            r.truncate(8);
        }
    }
    Some(Table {
        format: Format::AccessLog,
        headers,
        rows,
        notes,
    })
}

/// A prefix such as `web-1  | `, `[pod/web-0/app] ` or `app[123]: ` in front of a JSON object.
fn clean_source(prefix: &str) -> Option<String> {
    let p = prefix.trim_end();
    let last = p.chars().last()?;
    if !matches!(last, '|' | ']' | ':') {
        return None;
    }
    let s = p
        .trim_end_matches(['|', ':'])
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    (!s.is_empty() && s.len() <= 80).then(|| s.to_string())
}

fn prefixed_line(line: &str) -> Option<(String, Vec<(String, String)>)> {
    let at = line.find('{')?;
    let source = clean_source(&line[..at])?;
    let Value::Object(m) = serde_json::from_str::<Value>(line[at..].trim()).ok()? else {
        return None;
    };
    let pairs = m
        .into_iter()
        .map(|(k, v)| {
            let cell = match v {
                Value::Null => String::new(),
                Value::String(s) => s,
                other => other.to_string(),
            };
            (
                if k == "source" {
                    "_source".to_string()
                } else {
                    k
                },
                cell,
            )
        })
        .collect();
    Some((source, pairs))
}

pub fn parse_prefixed_jsonl(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < MIN_ROWS || !lines.iter().take(5).all(|l| prefixed_line(l).is_some()) {
        return None;
    }
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for l in &lines {
        match prefixed_line(l) {
            Some(r) => rows.push(r),
            None => {
                if notes.len() < 5 {
                    notes.push(l.chars().take(200).collect());
                }
            }
        }
    }
    if (lines.len() - rows.len()) * 20 > lines.len() {
        return None;
    }
    table_from_pairs(Format::PrefixedJsonl, Some("source"), rows, notes)
}
