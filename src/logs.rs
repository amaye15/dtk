//! Log formats that are tables in disguise: logfmt, web access logs, JSON lines that carry a
//! prefix (`docker compose logs`, `kubectl logs --prefix`), and plain text lines that start with a
//! timestamp (application logs, CI logs, syslog).
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

// ---- text logs -------------------------------------------------------------------------------

fn digits(b: &[u8], from: usize, n: usize) -> bool {
    b.len() >= from + n && b[from..from + n].iter().all(u8::is_ascii_digit)
}

/// `HH:MM:SS` at `i`, with optional fraction. Returns the index after it.
fn clock(b: &[u8], i: usize) -> Option<usize> {
    if !(digits(b, i, 2) && b.get(i + 2) == Some(&b':') && digits(b, i + 3, 2)) {
        return None;
    }
    let mut j = i + 5;
    if b.get(j) == Some(&b':') && digits(b, j + 1, 2) {
        j += 3;
        if matches!(b.get(j), Some(b'.') | Some(b',')) && digits(b, j + 1, 1) {
            j += 1;
            while digits(b, j, 1) {
                j += 1;
            }
        }
    }
    Some(j)
}

/// A date that Java and Hadoop loggers write, with the space after it: `26/07/13 ` (log4j's
/// `yy/MM/dd`, the default in Spark), `07/13/2026 `, and `13-Jul-2026 ` (`java.util.logging`,
/// Tomcat). Returns where the clock starts. A date with no clock after it is not a timestamp.
fn short_date(b: &[u8], i: usize) -> Option<usize> {
    let end = if digits(b, i, 2)
        && b.get(i + 2) == Some(&b'/')
        && digits(b, i + 3, 2)
        && b.get(i + 5) == Some(&b'/')
    {
        if digits(b, i + 6, 4) {
            i + 10
        } else if digits(b, i + 6, 2) {
            i + 8
        } else {
            return None;
        }
    } else if digits(b, i, 2)
        && b.get(i + 2) == Some(&b'-')
        && b.get(i + 6) == Some(&b'-')
        && digits(b, i + 7, 4)
        && b.get(i + 3..i + 6)
            .and_then(|m| std::str::from_utf8(m).ok())
            .is_some_and(|m| MONTHS.contains(&m))
    {
        i + 11
    } else {
        return None;
    };
    (b.get(end) == Some(&b' ')).then_some(end + 1)
}

/// The length in bytes of the timestamp a line starts with, brackets included, or `None`.
/// Forms: `2026-11-02T10:00:00.123Z`, `2026-11-02 10:00:00,123 +0100`, `2026/11/02 10:00:00`,
/// `Nov  2 10:00:00` (syslog), `11-02 10:00:00.123` (logcat), `26/07/13 22:58:59` (Spark),
/// `13-Jul-2026 22:58:59.123` (Tomcat), `[10:00:00]`, and any of these in square brackets.
fn leading_timestamp(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let open = usize::from(b.first() == Some(&b'['));
    let mut i = open;
    let end = if digits(b, i, 4)
        && matches!(b.get(i + 4), Some(b'-') | Some(b'/'))
        && digits(b, i + 5, 2)
        && digits(b, i + 8, 2)
    {
        i += 10;
        if matches!(b.get(i), Some(b'T') | Some(b' ')) {
            let j = clock(b, i + 1)?;
            i = j;
            if b.get(i) == Some(&b'Z') {
                i += 1;
            } else if matches!(b.get(i), Some(b'+') | Some(b'-')) && digits(b, i + 1, 2) {
                i += 3;
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                if digits(b, i, 2) {
                    i += 2;
                }
            }
        }
        i
    } else if digits(b, i, 2)
        && b.get(i + 2) == Some(&b'-')
        && digits(b, i + 3, 2)
        && b.get(i + 5) == Some(&b' ')
    {
        clock(b, i + 6)?
    } else if let Some(at) = short_date(b, i) {
        clock(b, at)?
    } else if line.get(i..i + 3).is_some_and(|m| MONTHS.contains(&m)) && b.get(i + 3) == Some(&b' ')
    {
        // `Nov  2 10:00:00` or `Nov 12 10:00:00`
        let mut j = i + 4;
        while b.get(j) == Some(&b' ') {
            j += 1;
        }
        if !digits(b, j, 1) {
            return None;
        }
        j += if digits(b, j, 2) { 2 } else { 1 };
        if b.get(j) != Some(&b' ') {
            return None;
        }
        clock(b, j + 1)?
    } else if open == 1 {
        // Only in brackets: a bare clock is too common at the start of a line.
        clock(b, i)?
    } else {
        return None;
    };
    if open == 1 {
        if b.get(end) == Some(&b']') {
            Some(end + 1)
        } else {
            None
        }
    } else {
        Some(end)
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A level at the start of the text after the timestamp: `INFO`, `[WARN]`, `ERROR:`, `<debug>`.
/// Returns the normalized level and what is left of the message.
fn leading_level(rest: &str) -> Option<(&'static str, &str)> {
    let rest = rest.trim_start();
    let (wrapped, body) = match rest.as_bytes().first()? {
        b'[' | b'<' | b'(' => (true, &rest[1..]),
        _ => (false, rest),
    };
    let end = body
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(body.len());
    let word = &body[..end];
    let after = &body[end..];
    let upper = word.chars().all(|c| c.is_ascii_uppercase());
    let colon = after.starts_with(':');
    let closed = wrapped && after.starts_with([']', '>', ')']);
    // Plain lowercase words ("error occurred") are prose; they count only when marked.
    if !(upper && (wrapped == closed) || (colon || closed)) {
        return None;
    }
    let level = match word.to_ascii_uppercase().as_str() {
        "TRACE" => "trace",
        "DEBUG" | "DBG" => "debug",
        "INFO" | "NOTICE" => "info",
        "WARN" | "WARNING" => "warn",
        "ERROR" | "ERR" | "SEVERE" => "error",
        "FATAL" | "CRITICAL" | "CRIT" | "PANIC" => "fatal",
        _ => return None,
    };
    let tail = after.trim_start_matches([']', '>', ')', ':']);
    Some((level, tail.trim_start_matches([' ', '-', '|'])))
}

/// Does the message report trouble? For logs that have no level field. Precision matters more
/// than recall here: a crate named `quick-error` or the phrase "network errors" in a step title
/// is not an error. So a word counts only when it leads the message or is a label: `error:`,
/// `error[E0308]`, `##[error]`, `FAILED`, `Traceback`, `panicked at`, `TypeError:`. A phrase such
/// as "0 errors" or "no failures" means the opposite, and is not counted.
fn flag_of(msg: &str) -> &'static str {
    let first = msg.lines().next().unwrap_or("").trim();
    let calm = [
        "0 error",
        "no error",
        "without error",
        "errors: 0",
        "error: 0",
        "0 failed",
        "no fail",
        "0 failure",
        "failures: 0",
        "failed: 0",
        "0 warning",
        "no warning",
        "warnings: 0",
        "warning: 0",
    ];
    let low = calm
        .iter()
        .fold(first.to_ascii_lowercase(), |s, c| s.replace(c, ""));
    let starts = |words: &[&str]| words.iter().any(|w| low.starts_with(w));
    // A whole upper-case token, as in "test result: FAILED. 3 passed" or "BUILD FAILED".
    let token = |words: &[&str]| {
        first
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|t| words.contains(&t))
    };
    let kind_error = first
        .split_whitespace()
        .any(|w| w.ends_with("Error:") || w.ends_with("Exception:") || w.ends_with("Exception"));
    if starts(&[
        "error",
        "fatal",
        "panic",
        "exception",
        "traceback",
        "failure",
        "failed",
        "fail:",
        "fail ",
        "##[error]",
        "npm err",
        "e: ",
        "caused by:",
        "--- fail",
        "abort",
    ]) || low.contains("panicked at")
        || low.contains("error[e")
        || low.contains(": error:")
        || low.contains("process completed with exit code") && !low.contains("exit code 0")
        || token(&["FAILED", "FAILURE", "ERROR", "FATAL", "PANIC"])
        || kind_error
    {
        "error"
    } else if starts(&["warn", "##[warning]", "npm warn", "w: ", "deprecat"])
        || low.contains(": warning:")
        || token(&["WARN", "WARNING", "DEPRECATED"])
    {
        "warn"
    } else {
        ""
    }
}

/// Plain text lines that start with a timestamp. A line that does not start with one (a stack
/// trace) belongs to the record above it.
pub fn parse_text_log(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let starts: Vec<Option<usize>> = lines.iter().map(|l| leading_timestamp(l)).collect();
    let records = starts.iter().filter(|s| s.is_some()).count();
    // Stack traces and wrapped lines belong to a record, so only a third of the lines need to
    // start one. What makes it a log is that the timestamps run forward.
    if records < 20 || records * 100 < lines.len() * 35 {
        return None;
    }
    let stamps: Vec<&str> = lines
        .iter()
        .zip(&starts)
        .filter_map(|(l, s)| s.map(|e| &l[..e]))
        .collect();
    let forward = stamps.windows(2).filter(|w| w[0] <= w[1]).count();
    if forward * 100 < (stamps.len() - 1) * 75 {
        return None;
    }
    // The log starts with a record (a short title above it is allowed).
    let first = starts.iter().position(Option::is_some)?;
    if first > 3 {
        return None;
    }
    let mut notes: Vec<String> = lines[..first]
        .iter()
        .map(|l| l.trim().to_string())
        .collect();
    // (timestamp, level, message without the level, message as written)
    let mut recs: Vec<(String, Option<&'static str>, String, String)> = Vec::with_capacity(records);
    for (line, start) in lines.iter().zip(&starts).skip(first) {
        match start {
            Some(end) => {
                let (ts, rest) = line.split_at(*end);
                let ts = ts.trim_matches(['[', ']']).to_string();
                let rest = rest.trim_start();
                let full = rest.trim_end().to_string();
                match leading_level(rest) {
                    Some((lv, msg)) => recs.push((ts, Some(lv), msg.trim_end().to_string(), full)),
                    None => recs.push((ts, None, full.clone(), full)),
                }
            }
            None => {
                if let Some(last) = recs.last_mut() {
                    for m in [&mut last.2, &mut last.3] {
                        m.push('\n');
                        m.push_str(line.trim_end());
                    }
                }
            }
        }
    }
    let with_level = recs.iter().filter(|r| r.1.is_some()).count();
    let explicit = with_level * 2 >= recs.len();
    let mut headers = vec!["ts".to_string()];
    if explicit {
        headers.push("level".into());
    }
    headers.push("msg".into());
    // With a level field the message loses its level word; without one the message stays as it
    // was written, so that "warning: ..." is still there for the flag to find.
    let flags: Vec<&str> = recs.iter().map(|r| flag_of(&r.3)).collect();
    let guess = !explicit && flags.iter().any(|f| !f.is_empty());
    if guess {
        headers.push("flag".into());
        notes.push("flag is dtk's guess from words such as error, failed and warning in the message; this log has no level field".into());
    }
    let rows: Vec<Vec<String>> = recs
        .into_iter()
        .zip(flags)
        .map(|((ts, lv, msg, full), flag)| {
            let mut row = vec![ts];
            if explicit {
                row.push(lv.unwrap_or("").to_string());
            }
            row.push(if explicit { msg } else { full });
            if guess {
                row.push(flag.to_string());
            }
            row
        })
        .collect();
    notes.truncate(5);
    Some(Table {
        format: Format::TextLog,
        headers,
        rows,
        notes,
    })
}

#[cfg(test)]
mod text_log_tests {
    use super::*;

    #[test]
    fn finds_timestamps_in_the_usual_forms() {
        for (line, len) in [
            ("2026-11-02T10:00:00.1234567Z hello", 28),
            ("2026-11-02 10:00:00,123 INFO x", 23),
            ("2026-11-02 10:00:00+0100 x", 24),
            ("2026/11/02 10:00:00 x", 19),
            ("Nov  2 10:00:00 host x", 15),
            ("Nov 12 10:00:00 host x", 15),
            ("11-02 10:00:00.123  1234 I Tag", 18),
            ("[2026-11-02 10:00:00] x", 21),
            ("[10:00:00.5] x", 12),
            ("26/07/13 22:58:59 INFO SparkContext: x", 17),
            ("07/13/2026 22:58:59,123 INFO x", 23),
            ("13-Jul-2026 22:58:59.123 INFO [main] x", 24),
            ("[26/07/13 22:58:59] x", 19),
        ] {
            assert_eq!(leading_timestamp(line), Some(len), "{line}");
        }
    }

    #[test]
    fn does_not_mistake_other_lines_for_timestamps() {
        for line in [
            "10:00:00 a bare clock is too common",
            "2026-11-02 is a date with no time",
            "May the force be with you",
            "Nov is short for November",
            "version 2026-11-02T10:00:00Z later",
            "[note] 2026-11-02T10:00:00Z",
            "1.2.3 10:00:00",
            "26/07/13 is a date with no time",
            "1/2/3 10:00:00",
            "13-Jul-2026 is a date with no time",
            "13-Foo-2026 22:58:59",
            "26/07/1 22:58:59",
        ] {
            assert_eq!(leading_timestamp(line), None, "{line}");
        }
    }

    #[test]
    fn multibyte_text_never_panics() {
        // Byte offsets of ASCII patterns can land inside a multi-byte character.
        for line in [
            "┆┆┆ x",
            "é\u{301}ab 1",
            "[é] x",
            "Növ 2 10:00:00",
            "日本語の行です",
            "[\u{fffd}\u{fffd}",
            "2026-11-0é 10:00",
            "2026-11-02T10:00:0é",
            "<é> x",
            "[é]: y",
        ] {
            let _ = leading_timestamp(line);
            let _ = leading_level(line);
            let _ = flag_of(line);
        }
        let log: String = (0..40)
            .map(|i| format!("2026-11-02T10:00:{i:02}Z ┆ naïve café {i} — fin\n"))
            .collect();
        for cut in (0..log.len()).filter(|c| log.is_char_boundary(*c)) {
            let _ = parse_text_log(&log[..cut]);
        }
        assert!(parse_text_log(&log).is_some());
    }

    #[test]
    fn reads_levels_and_leaves_prose_alone() {
        assert_eq!(leading_level("INFO started"), Some(("info", "started")));
        assert_eq!(leading_level("[WARN] disk low"), Some(("warn", "disk low")));
        assert_eq!(leading_level("ERROR: boom"), Some(("error", "boom")));
        assert_eq!(leading_level("<debug> x"), Some(("debug", "x")));
        assert_eq!(leading_level("WARNING - slow"), Some(("warn", "slow")));
        assert_eq!(leading_level("error occurred in parse"), None);
        assert_eq!(leading_level("INFORMATION desk"), None);
        assert_eq!(leading_level("Started server"), None);
    }

    #[test]
    fn flags_trouble_words_but_not_their_negations() {
        for (msg, flag) in [
            ("##[error]Process completed with exit code 1.", "error"),
            ("Process completed with exit code 101", "error"),
            ("test result: FAILED. 3 passed; 1 failed", "error"),
            ("error[E0308]: mismatched types", "error"),
            ("Error: connection refused", "error"),
            ("thread 'main' panicked at src/main.rs:3", "error"),
            ("Traceback (most recent call last):", "error"),
            ("ValueError: bad input", "error"),
            ("BUILD FAILED in 4s", "error"),
            ("src/a.c:3:5: error: expected ';'", "error"),
            ("warning: unused variable", "warn"),
            ("##[warning]Node.js 20 is deprecated", "warn"),
            ("npm WARN deprecated left-pad@1.0.0", "warn"),
            ("Finished with 0 errors, 0 warnings", ""),
            ("no failures found", ""),
            ("Downloaded quick-error v2.0.1", ""),
            ("Compiling quick-error v2.0.1", ""),
            ("Work around spurious network errors", ""),
            ("terror strikes", ""),
            ("Process completed with exit code 0", ""),
            ("Compiling dtk v0.4.0", ""),
            ("hint: to use in all of your new repositories", ""),
        ] {
            assert_eq!(flag_of(msg), flag, "{msg}");
        }
    }

    fn app_log(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            let lv = ["INFO", "INFO", "WARN", "ERROR"][i % 4];
            s.push_str(&format!(
                "2026-11-02 10:{:02}:{:02},{:03} {lv} [worker-{}] request {i} done in {}ms\n",
                i / 60,
                i % 60,
                i % 1000,
                i % 3,
                i * 7 % 500
            ));
            if lv == "ERROR" {
                s.push_str("java.lang.IllegalStateException: boom\n\tat com.example.Service.run(Service.java:42)\n\tat com.example.Main.main(Main.java:7)\n");
            }
        }
        s
    }

    #[test]
    fn reads_an_application_log_with_stack_traces() {
        let t = parse_text_log(&app_log(60)).expect("app log");
        assert_eq!(t.format, Format::TextLog);
        assert_eq!(t.headers, ["ts", "level", "msg"]);
        assert_eq!(t.rows.len(), 60);
        assert_eq!(t.rows[0][1], "info");
        assert_eq!(t.rows[3][1], "error");
        assert!(
            t.rows[3][2].contains("IllegalStateException"),
            "{}",
            t.rows[3][2]
        );
        assert!(t.rows[3][2].contains("Main.java:7"));
        assert!(!t.rows[4][2].contains("IllegalState"));
    }

    #[test]
    fn guesses_a_flag_when_the_log_has_no_level() {
        let mut s = String::new();
        for i in 0..40 {
            let msg = match i % 10 {
                3 => "error[E0308]: mismatched types".to_string(),
                6 => "warning: unused import".to_string(),
                _ => format!("   Compiling crate{i} v0.1.{i}"),
            };
            s.push_str(&format!("2026-11-02T02:25:{:02}.1234567Z {msg}\n", i));
        }
        let t = parse_text_log(&s).expect("ci log");
        assert_eq!(t.headers, ["ts", "msg", "flag"]);
        assert_eq!(t.rows[3][2], "error");
        assert_eq!(t.rows[6][2], "warn");
        assert_eq!(t.rows[0][2], "");
        assert!(t.notes.iter().any(|n| n.contains("dtk's guess")));
    }

    #[test]
    fn no_flag_column_when_nothing_is_flagged() {
        let s: String = (0..30)
            .map(|i| format!("2026-11-02T02:25:{:02}Z step {i} ok\n", i))
            .collect();
        let t = parse_text_log(&s).unwrap();
        assert_eq!(t.headers, ["ts", "msg"]);
    }

    #[test]
    fn reads_syslog() {
        let s: String = (0..30)
            .map(|i| {
                format!(
                    "Nov  2 10:{:02}:00 host sshd[{}]: Accepted publickey for dev\n",
                    i,
                    100 + i
                )
            })
            .collect();
        let t = parse_text_log(&s).unwrap();
        assert_eq!(t.rows.len(), 30);
        assert_eq!(t.rows[1][0], "Nov  2 10:01:00");
    }

    #[test]
    fn ordinary_text_is_not_a_log() {
        let prose = "2026-11-02 was a good day.\nWe went out.\n".repeat(30);
        assert!(
            parse_text_log(&prose).is_none(),
            "a date alone is not a timestamp"
        );
        // Timestamps that jump around are a list of events, not a log.
        let shuffled: String = (0..40)
            .map(|i| format!("2026-11-02T{:02}:00:00Z event {i}\n", (i * 7) % 24))
            .collect();
        assert!(parse_text_log(&shuffled).is_none());
        // A few timestamps in a long document.
        let doc: String = (0..40)
            .map(|i| {
                if i % 8 == 0 {
                    format!("2026-11-02T10:00:{i:02}Z checkpoint\n")
                } else {
                    format!("some paragraph of an ordinary report, line {i}\n")
                }
            })
            .collect();
        assert!(parse_text_log(&doc).is_none());
        let few: String = (0..10)
            .map(|i| format!("2026-11-02T10:00:{i:02}Z x\n"))
            .collect();
        assert!(
            parse_text_log(&few).is_none(),
            "ten lines are too few to call a log"
        );
    }
}
