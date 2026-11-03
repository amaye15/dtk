//! Table detection: turns raw command output into a header plus rows, or says "not a table".
//!
//! Every parser is strict on purpose. A false positive replaces real output with a summary,
//! so unsure input must return `None`.

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Fewest data rows that count as a table.
pub const MIN_ROWS: usize = 10;
/// Most columns a table may have.
const MAX_COLS: usize = 150;
/// Largest input the parsers accept.
pub const MAX_INPUT: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jsonl,
    JsonArray,
    Csv,
    Tsv,
    Aligned,
    /// Tables drawn with borders: polars, psql, MySQL, SQLite, Markdown.
    Bordered,
    /// An object that wraps one big array of objects, or an object keyed by id.
    JsonObject,
    /// `key=value` lines.
    Logfmt,
    /// Apache or nginx access log lines.
    AccessLog,
    /// JSON lines with a prefix such as `web-1  | ` in front of each object.
    PrefixedJsonl,
    /// Whitespace table with a free-text last column: `ps aux`, `df -h`.
    TailText,
    /// Plain text lines that start with a timestamp.
    TextLog,
    /// A Python list of dicts as `print` shows it.
    PyRepr,
    /// Rows of numbers with no header: a printed matrix.
    Grid,
    /// Delimited text with no header row.
    Headerless,
    /// One block per row: `psql -x`, `sqlite3 -line`, MySQL `\G`.
    Records,
    /// A page that is one HTML table.
    Html,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Jsonl => "jsonl",
            Format::JsonArray => "json-array",
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Aligned | Format::Bordered | Format::TailText => "table",
            Format::TextLog => "text-log",
            Format::PyRepr => "python-repr",
            Format::Grid => "number-grid",
            Format::Headerless => "delimited",
            Format::Records => "records",
            Format::Html => "html-table",
            Format::JsonObject => "json-object",
            Format::Logfmt => "logfmt",
            Format::AccessLog => "access-log",
            Format::PrefixedJsonl => "jsonl-prefixed",
        }
    }
}

#[derive(Debug)]
pub struct Table {
    pub format: Format,
    pub headers: Vec<String>,
    /// Every row has exactly `headers.len()` cells.
    pub rows: Vec<Vec<String>>,
    /// Lines that are not rows (footers, bad lines), kept verbatim.
    pub notes: Vec<String>,
}

/// Returns a table when `text` is clearly one.
pub fn detect(text: &str) -> Option<Table> {
    detect_lines(text).and_then(|t| accounted_for(t, text))
}

/// A reader may set aside a few lines that are not rows (a footer, a bad line). When more than a
/// few are left over, the text is not a table: it is a document with a table in it, or a list
/// that happens to align, and replacing it would drop most of it without a word. Fewer lines
/// are fine, but the summary says that they are not shown.
fn accounted_for(mut t: Table, text: &str) -> Option<Table> {
    let (header, borders) = match t.format {
        Format::Aligned | Format::TailText => (1, false),
        Format::Bordered => (1, true),
        Format::Jsonl
        | Format::PrefixedJsonl
        | Format::Logfmt
        | Format::AccessLog
        | Format::TextLog => (0, false),
        // One row may take several lines (CSV with a quoted newline, record blocks), or the row
        // is not a line at all (JSON, HTML, a Python value).
        _ => return Some(t),
    };
    let nonblank = text.lines().filter(|l| !l.trim().is_empty());
    let lines = if borders {
        nonblank.filter(|l| !is_border(l)).count()
    } else {
        nonblank.count()
    };
    let shown = t.rows.len() + header + t.notes.len();
    let left = lines.saturating_sub(shown);
    if left > 5 && left * 10 > t.rows.len() {
        return None;
    }
    if left > 0 {
        t.notes
            .push(format!("{left} other lines are not rows and are not shown"));
    }
    Some(t)
}

fn detect_lines(text: &str) -> Option<Table> {
    if text.len() > MAX_INPUT || is_long_listing(text) {
        return None;
    }
    let cleaned = clean(text);
    // The spaces before the first line align it with the columns under it: pandas output with a
    // MultiIndex is read by position, so that one line must keep them.
    let aligned = cleaned.trim_start_matches(['\n', '\r']).trim_end();
    let text = skip_comment_lines(cleaned.trim());
    if text.len() < 200 {
        return None;
    }
    match text.as_bytes()[0] {
        b'[' => {
            if let Some(t) = parse_json_array(text) {
                return Some(t);
            }
            if let Some(t) = serde_json::from_str::<Value>(text).ok().and_then(|v| {
                crate::foreign::parse_columnar(&v).or_else(|| crate::foreign::parse_nested_json(&v))
            }) {
                return Some(t);
            }
            if let Some(t) = crate::foreign::parse_python_repr(text) {
                return Some(t);
            }
        }
        b'{' => {
            // A dict per column (`df.to_json()`) must be tried first: with ten columns or more,
            // the container reader would take the columns for rows.
            if let Some(t) = serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|v| crate::foreign::parse_nested_json(&v))
            {
                return Some(t);
            }
            return parse_jsonl(text)
                .or_else(|| parse_json_container(text))
                .or_else(|| {
                    serde_json::from_str::<Value>(text)
                        .ok()
                        .and_then(|v| crate::foreign::parse_columnar(&v))
                })
                .or_else(|| crate::foreign::parse_python_repr(text));
        }
        b'<' => return crate::foreign::parse_html_table(text),
        _ => {}
    }
    crate::logs::parse_access_log(text)
        .or_else(|| crate::logs::parse_prefixed_jsonl(text))
        .or_else(|| crate::logs::parse_logfmt(text))
        .or_else(|| parse_bordered(text))
        .or_else(|| parse_delimited(text))
        .or_else(|| crate::multiindex::parse_multiindex(aligned))
        .or_else(|| parse_aligned(text))
        .or_else(|| crate::multiindex::parse_right_aligned(aligned))
        .or_else(|| crate::textcols::parse_tail_text(text))
        .or_else(|| crate::foreign::parse_number_grid(text))
        .or_else(|| crate::foreign::parse_records(text))
        .or_else(|| crate::logs::parse_text_log(text))
        .or_else(|| crate::foreign::parse_headerless(text))
}

/// A table with a few lines of prose around it: a status line before a JSON array, a title above
/// a CSV, a database server's warning on both sides of its rows.
pub struct Wrapped {
    pub table: Table,
    /// The data is `text[start..end]`; the text before and after is kept as it is.
    pub start: usize,
    pub end: usize,
}

/// At most this many lines of prose before or after the data, each at most this long.
const WRAP_LINES: usize = 6;
const WRAP_LINE_CHARS: usize = 300;

fn is_prose_block(s: &str) -> bool {
    s.lines().count() <= WRAP_LINES && s.lines().all(|l| l.chars().count() <= WRAP_LINE_CHARS)
}

/// Where the first JSON value in `s` ends, as a byte offset. `s` starts with `[` or `{`.
fn json_value_end(s: &str) -> Option<usize> {
    let mut it = serde_json::Deserializer::from_str(s).into_iter::<Value>();
    it.next()?.ok()?;
    Some(it.byte_offset())
}

/// Do ten lines in a row have the same number (one or more) of some delimiter, or the same
/// number (two or more) of gaps of two spaces? Cheap evidence that a table starts here.
fn looks_tabular(rest: &str) -> bool {
    let lines: Vec<&str> = rest
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(MIN_ROWS + 1)
        .collect();
    if lines.len() < MIN_ROWS + 1 {
        return false;
    }
    let same = |count: &dyn Fn(&str) -> usize, least: usize| {
        let first = count(lines[0]);
        first >= least && lines.iter().filter(|l| count(l) == first).count() > MIN_ROWS - 1
    };
    [',', '\t', ';', '|']
        .iter()
        .any(|d| same(&|l: &str| l.matches(*d).count(), 1))
        || same(&|l: &str| l.trim().split("  ").count() - 1, 2)
}

/// Finds a table that has prose around it. Only tried when `detect` finds nothing in the text
/// as it is.
pub fn detect_wrapped(text: &str) -> Option<Wrapped> {
    if text.len() > MAX_INPUT || text.len() < 200 {
        return None;
    }
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
            if starts.len() > WRAP_LINES + 1 {
                break;
            }
        }
    }
    for (p, &off) in starts.iter().enumerate().take(WRAP_LINES + 1) {
        if p > 0 && !is_prose_block(&text[..off]) {
            return None;
        }
        let rest = &text[off..];
        let body = rest.trim_start();
        let body_at = off + (rest.len() - body.len());
        let first = body.as_bytes().first().copied();
        // The rows start here, as JSON lines or CSV: let the plain readers have the rest.
        if p > 0 && (matches!(first, Some(b'[' | b'{')) || looks_tabular(rest)) {
            if let Some(table) = detect(rest) {
                return Some(Wrapped {
                    table,
                    start: off,
                    end: text.len(),
                });
            }
        }
        // One JSON value, then a few lines of prose.
        if matches!(first, Some(b'[' | b'{')) {
            if let Some(len) = json_value_end(body) {
                let after = &body[len..];
                if (p > 0 || !after.trim().is_empty())
                    && is_prose_block(after.trim_start_matches('\n'))
                {
                    if let Some(table) = detect(&body[..len]) {
                        return Some(Wrapped {
                            table,
                            start: body_at,
                            end: body_at + len,
                        });
                    }
                }
            }
        }
    }
    None
}

/// The objects of a JSON array, or the entries of a JSON object whose values are objects.
enum Rows<'a> {
    Array(Vec<&'a Map<String, Value>>),
    Keyed(Vec<(&'a String, &'a Map<String, Value>)>),
}

impl Rows<'_> {
    fn len(&self) -> usize {
        match self {
            Rows::Array(v) => v.len(),
            Rows::Keyed(v) => v.len(),
        }
    }
}

fn rows_of(v: &Value) -> Option<Rows<'_>> {
    match v {
        Value::Array(a) => {
            let objs: Vec<_> = a.iter().filter_map(Value::as_object).collect();
            (objs.len() >= MIN_ROWS && objs.len() * 20 >= a.len() * 19).then_some(Rows::Array(objs))
        }
        Value::Object(m) => {
            let kv: Vec<_> = m
                .iter()
                .filter_map(|(k, x)| x.as_object().map(|o| (k, o)))
                .collect();
            (kv.len() >= MIN_ROWS && kv.len() * 20 >= m.len() * 19).then_some(Rows::Keyed(kv))
        }
        _ => None,
    }
}

fn scalar_note(k: &str, v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(format!("{k}: {}", s.chars().take(80).collect::<String>())),
        Value::Number(_) | Value::Bool(_) => Some(format!("{k}: {v}")),
        _ => None,
    }
}

/// A JSON object that wraps one big array of objects (`{"items": [...]}`, as `kubectl` and many
/// REST APIs return), or that is keyed by id. The array is looked for at most two levels down.
/// Two similar candidates are ambiguous, so the answer is then no table.
fn parse_json_container(text: &str) -> Option<Table> {
    let root: Value = serde_json::from_str(text).ok()?;
    let top = root.as_object()?;
    let mut cands: Vec<(Vec<&str>, Rows)> = Vec::new();
    if let Some(r) = rows_of(&root) {
        cands.push((Vec::new(), r));
    }
    for (k, v) in top {
        if let Some(r) = rows_of(v) {
            cands.push((vec![k.as_str()], r));
        } else if let Some(inner) = v.as_object() {
            for (k2, v2) in inner {
                if let Some(r) = rows_of(v2) {
                    cands.push((vec![k.as_str(), k2.as_str()], r));
                }
            }
        }
    }
    cands.sort_by_key(|c| std::cmp::Reverse(c.1.len()));
    if cands.is_empty() || (cands.len() > 1 && cands[0].1.len() < cands[1].1.len() * 3) {
        return None;
    }
    let (path, rows) = cands.remove(0);
    // The rows must be most of the document. If a small array sits beside a big object (a plan
    // with 40 outputs and 120 resources somewhere else), summarizing the array would silently
    // drop the rest of the document. That is a job for an outline, not for a table.
    let rows_bytes: usize = match &rows {
        Rows::Array(v) => v
            .iter()
            .map(|o| serde_json::to_string(o).map_or(0, |s| s.len()))
            .sum(),
        Rows::Keyed(v) => v
            .iter()
            .map(|(k, o)| k.len() + serde_json::to_string(o).map_or(0, |s| s.len()))
            .sum(),
    };
    let total_bytes = serde_json::to_string(&root).ok()?.len();
    if rows_bytes * 10 < total_bytes * 7 {
        return None;
    }
    // Scalars next to the table (total_count, status, page...) are kept as notes.
    let mut notes = Vec::new();
    let mut level = Some(&root);
    for depth in 0..=path.len() {
        if let Some(m) = level.and_then(Value::as_object) {
            for (k, v) in m {
                if path.get(depth).is_some_and(|p| p == k) {
                    continue;
                }
                if let Some(n) = scalar_note(k, v) {
                    if notes.len() < 5 {
                        notes.push(n);
                    }
                }
            }
        }
        level = path.get(depth).and_then(|p| level.and_then(|l| l.get(*p)));
    }
    match rows {
        Rows::Array(objs) => from_objects(Format::JsonObject, objs.into_iter(), notes),
        Rows::Keyed(kv) => from_objects(Format::JsonObject, keyed_maps(&kv).iter(), notes),
    }
}

/// Rows keyed by id (`{"a1": {...}, "a2": {...}}`) as objects that carry the key in a `key` field.
fn keyed_maps(kv: &[(&String, &Map<String, Value>)]) -> Vec<Map<String, Value>> {
    kv.iter()
        .map(|(k, o)| {
            let mut m = Map::new();
            m.insert("key".to_string(), Value::String((*k).clone()));
            for (kk, vv) in o.iter() {
                let name = if kk == "key" {
                    "_key".to_string()
                } else {
                    kk.clone()
                };
                m.insert(name, vv.clone());
            }
            m
        })
        .collect()
}

/// A table from an object keyed by row label, read from something other than JSON text
/// (`df.to_dict("index")`).
pub(crate) fn from_keyed_root(format: Format, root: &Value) -> Option<Table> {
    match rows_of(root)? {
        Rows::Keyed(kv) => from_objects(format, keyed_maps(&kv).iter(), Vec::new()),
        Rows::Array(_) => None,
    }
}

/// Removes what makes a table unreadable to a parser but invisible to a person: a byte-order mark
/// and terminal colour codes (`jq -C`, `rich`, `ls --color`).
fn clean(text: &str) -> std::borrow::Cow<'_, str> {
    let text = text.trim_start_matches('\u{feff}');
    if !text.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ parameters, then one final byte in @..~
            Some('[') => {
                chars.next();
                for f in chars.by_ref() {
                    if ('@'..='~').contains(&f) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL, or ESC \
            Some(']') => {
                chars.next();
                while let Some(f) = chars.next() {
                    if f == '\u{7}' || (f == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            // Any other two-byte escape.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Leading `#` lines are a comment header (some CSV and TSV exports have one). Skip them.
fn skip_comment_lines(text: &str) -> &str {
    let mut rest = text;
    let mut skipped = false;
    while rest.starts_with('#') {
        match rest.split_once('\n') {
            Some((_, tail)) => {
                rest = tail.trim_start_matches(['\r', '\n']);
                skipped = true;
            }
            None => return text,
        }
    }
    if skipped {
        rest
    } else {
        text
    }
}

/// A character that belongs to a table border or rule.
fn is_border_char(c: char) -> bool {
    "+-=|: ".contains(c) || ('\u{2500}'..='\u{257F}').contains(&c)
}

/// A line of only dashes or equals signs and spaces, such as the rule under a header
/// (`--  -----`). It is not data.
pub(crate) fn is_rule_line(line: &str) -> bool {
    let t = line.trim();
    t.chars().filter(|c| *c == '-' || *c == '=').count() >= 2
        && t.chars().all(|c| c == '-' || c == '=' || c == ' ')
}

/// Characters that separate cells.
const CELL_SEPS: [char; 6] = ['|', '│', '┃', '║', '┆', '┊'];

/// A rule line such as `+----+----+`, `|---|---|`, `----+----` or `┌────┬────┐`.
fn is_border(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 3
        && t.chars().all(is_border_char)
        && t.chars()
            .any(|c| "-─━═=".contains(c) || ('\u{2500}'..='\u{257F}').contains(&c))
}

fn split_bordered(line: &str) -> Vec<String> {
    let t = line.trim();
    let mut cells: Vec<String> = t
        .split(|c| CELL_SEPS.contains(&c))
        .map(|s| s.trim().to_string())
        .collect();
    if t.starts_with(|c| CELL_SEPS.contains(&c)) {
        cells.remove(0);
    }
    if t.ends_with(|c| CELL_SEPS.contains(&c)) {
        cells.pop();
    }
    cells
}

/// Tables with borders. A rule line is required, which keeps shell pipelines and prose out.
fn parse_bordered(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text.lines().collect();
    if !lines.iter().any(|l| is_border(l)) {
        return None;
    }
    let is_content = |l: &str| !l.trim().is_empty() && !is_border(l) && l.contains(CELL_SEPS);
    let header_at = lines.iter().position(|l| is_content(l))?;
    let headers = split_bordered(lines[header_at]);
    if !valid_header(&headers) {
        return None;
    }
    let mut notes: Vec<String> = Vec::new();
    for l in &lines[..header_at] {
        if !l.trim().is_empty() && !is_border(l) && notes.len() < 5 {
            notes.push(l.trim().to_string());
        }
    }
    let mut i = header_at + 1;
    // polars prints a "---" row and a data-type row under the header, then a ═ rule.
    // Everything up to that rule is metadata, not data.
    let rule = (header_at + 1..lines.len().min(header_at + 5))
        .find(|&k| is_border(lines[k]) && lines[k].contains(['═', '╪']));
    if let Some(k) = rule {
        i = k + 1;
    }
    let mut rows = Vec::new();
    let mut bad = 0usize;
    for l in &lines[i..] {
        if l.trim().is_empty() || is_border(l) {
            continue;
        }
        if !is_content(l) {
            if notes.len() < 5 {
                notes.push(l.trim().to_string());
            }
            continue;
        }
        let cells = split_bordered(l);
        if cells.iter().all(|c| c == "…" || c == "...") {
            if notes.len() < 5 {
                notes.push("(the printer elided rows here)".to_string());
            }
        } else if cells.len() == headers.len() {
            rows.push(cells);
        } else {
            bad += 1;
        }
    }
    if rows.len() < MIN_ROWS || bad * 20 > rows.len() {
        return None;
    }
    Some(Table {
        format: Format::Bordered,
        headers,
        rows,
        notes,
    })
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// How deep nested objects are opened into columns: `owner.login`, `a.b.c`.
const MAX_FLATTEN_DEPTH: usize = 2;
/// An object with more keys than this is a map (keys are data), not a record, and stays one cell.
const MAX_FLATTEN_KEYS: usize = 20;

/// Which nested objects to open into columns: those that most rows have, that always have the
/// same few keys, and are never mixed with strings or numbers in other rows.
fn flatten_plan(
    maps: &[&Map<String, Value>],
    prefix: &str,
    depth: usize,
    plan: &mut HashSet<String>,
) {
    if depth >= MAX_FLATTEN_DEPTH {
        return;
    }
    let mut keys: Vec<&str> = Vec::new();
    for m in maps {
        for (k, v) in m.iter() {
            if v.is_object() && !keys.contains(&k.as_str()) {
                keys.push(k);
            }
        }
    }
    for k in keys {
        if k.contains('.') {
            continue;
        }
        let subs: Vec<&Map<String, Value>> = maps
            .iter()
            .filter_map(|m| m.get(k).and_then(Value::as_object))
            .collect();
        let mixed = maps
            .iter()
            .any(|m| m.get(k).is_some_and(|v| !v.is_object() && !v.is_null()));
        let mut union: HashSet<&str> = HashSet::new();
        for sub in &subs {
            union.extend(sub.keys().map(String::as_str));
        }
        if mixed
            || subs.len() * 2 < maps.len()
            || union.is_empty()
            || union.len() > MAX_FLATTEN_KEYS
            || union.iter().any(|u| u.contains('.'))
        {
            continue;
        }
        let path = if prefix.is_empty() {
            k.to_string()
        } else {
            format!("{prefix}.{k}")
        };
        plan.insert(path.clone());
        flatten_plan(&subs, &path, depth + 1, plan);
    }
}

fn flatten_into(
    prefix: &str,
    m: &Map<String, Value>,
    plan: &HashSet<String>,
    out: &mut Vec<(String, String)>,
) {
    for (k, v) in m {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Object(sub) if plan.contains(&path) => flatten_into(&path, sub, plan, out),
            Value::Null if plan.contains(&path) => {}
            _ => out.push((path, cell(v))),
        }
    }
}

/// Columns and rows from objects, opened by `plan`. `None` when there are too many columns.
fn rows_of_objects(
    objects: &[&Map<String, Value>],
    plan: &HashSet<String>,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let mut headers: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut sparse: Vec<Vec<(usize, String)>> = Vec::with_capacity(objects.len());
    let mut pairs: Vec<(String, String)> = Vec::new();
    for obj in objects {
        pairs.clear();
        flatten_into("", obj, plan, &mut pairs);
        let mut row = Vec::with_capacity(pairs.len());
        for (k, v) in pairs.drain(..) {
            let i = match index.get(&k) {
                Some(i) => *i,
                None => {
                    if headers.len() >= MAX_COLS {
                        return None;
                    }
                    headers.push(k.clone());
                    index.insert(k, headers.len() - 1);
                    headers.len() - 1
                }
            };
            row.push((i, v));
        }
        sparse.push(row);
    }
    let n = headers.len();
    let rows = sparse
        .into_iter()
        .map(|r| {
            let mut out = vec![String::new(); n];
            for (i, s) in r {
                out[i] = s;
            }
            out
        })
        .collect();
    Some((headers, rows))
}

/// Builds a table from JSON objects. Columns are the union of keys in first-seen order. Nested
/// objects that are records (an `owner` with a `login` and an `id`) become columns of their own
/// (`owner.login`, `owner.id`), so that each field has its own statistics instead of one cell of
/// JSON text.
fn from_objects<'a>(
    format: Format,
    objects: impl Iterator<Item = &'a Map<String, Value>>,
    notes: Vec<String>,
) -> Option<Table> {
    let objs: Vec<&Map<String, Value>> = objects.collect();
    if objs.len() < MIN_ROWS {
        return None;
    }
    let mut plan = HashSet::new();
    flatten_plan(&objs, "", 0, &mut plan);
    // Fall back to the plain table if opening the objects makes more columns than we allow.
    let (headers, rows) =
        rows_of_objects(&objs, &plan).or_else(|| rows_of_objects(&objs, &HashSet::new()))?;
    if headers.len() < 2 {
        return None;
    }
    Some(Table {
        format,
        headers,
        rows,
        notes,
    })
}

/// A table from a list of JSON objects that were read from something other than JSON text.
pub(crate) fn from_object_list(format: Format, objects: &[Map<String, Value>]) -> Option<Table> {
    from_objects(format, objects.iter(), Vec::new())
}

fn parse_jsonl(text: &str) -> Option<Table> {
    let mut objects: Vec<Map<String, Value>> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut bad = 0usize;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        total += 1;
        match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(m)) => objects.push(m),
            _ => {
                bad += 1;
                if bad > 50 {
                    return None;
                }
                if notes.len() < 5 {
                    notes.push(l.chars().take(200).collect());
                }
            }
        }
    }
    if bad * 20 > total {
        return None;
    }
    from_objects(Format::Jsonl, objects.iter(), notes)
}

fn parse_json_array(text: &str) -> Option<Table> {
    let v: Value = serde_json::from_str(text).ok()?;
    let arr = v.as_array()?;
    let objects: Vec<&Map<String, Value>> = arr.iter().filter_map(|x| x.as_object()).collect();
    if objects.len() * 20 < arr.len() * 19 {
        return None;
    }
    from_objects(Format::JsonArray, objects.into_iter(), Vec::new())
}

/// Is this header cell really a piece of a data line? A number, then a space, then more
/// (`8 andrewmayes`, `256 Oct 25 13:50 name`); a date; half of a bracket. A column name that starts
/// with a number has no space after it (`5m`, `2024`).
fn starts_like_data(cell: &str) -> bool {
    let c = cell.trim().trim_start_matches('\u{feff}');
    let b = c.as_bytes();
    // A date or a timestamp (`2026-08-04T22:01:17Z ...`) is a value, whatever comes after it.
    let dated = b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit);
    // `name (128` and `)` are the two halves of `name (128, 1)` cut at its comma.
    let unbalanced = c.matches('(').count() != c.matches(')').count()
        || c.matches('[').count() != c.matches(']').count();
    dated
        || unbalanced
        || (c.starts_with(|ch: char| ch.is_ascii_digit()) && c.contains(char::is_whitespace))
}

/// Lines such as `-rw-r--r--  1 user  staff  530 Oct 26 20:22 name`: an `ls -l` listing. The
/// names are the content of a listing, and a summary would hide them.
fn is_long_listing(text: &str) -> bool {
    let perm = |l: &str| {
        let b = l.trim_start().as_bytes();
        b.len() > 10
            && b"-dlcbps".contains(&b[0])
            && b[1..10].iter().all(|c| b"rwxsStT-".contains(c))
            && (b[10] == b' ' || b"@+.".contains(&b[10]))
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.len() >= MIN_ROWS && lines.iter().filter(|l| perm(l)).count() * 10 >= lines.len() * 8
}

pub(crate) fn valid_header(h: &[String]) -> bool {
    if h.len() < 2 || h.len() > MAX_COLS {
        return false;
    }
    let mut seen = HashSet::new();
    for c in h {
        let c = c.trim();
        // Quotes and braces are what a line of JSON is made of: a header like `{"id":` is a data
        // line read with the wrong parser.
        if c.is_empty()
            || c.chars().count() > 60
            || c.split_whitespace().count() > 6
            || c.parse::<f64>().is_ok()
            || c.contains(['"', '{', '}'])
            || starts_like_data(c)
        {
            return false;
        }
        if !seen.insert(c.to_string()) {
            return false;
        }
    }
    true
}

/// Most common value of `counts` with how many times it occurs.
pub(crate) fn mode(counts: &[usize]) -> Option<(usize, usize)> {
    let mut freq: HashMap<usize, usize> = HashMap::new();
    for c in counts {
        *freq.entry(*c).or_default() += 1;
    }
    freq.into_iter().max_by_key(|(k, v)| (*v, *k))
}

fn try_delim(text: &str, d: u8) -> Option<Table> {
    let first = text.lines().next()?;
    if !first.as_bytes().contains(&d) {
        return None;
    }
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(d)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut recs: Vec<Vec<String>> = Vec::new();
    for r in rdr.records() {
        let rec = r.ok()?;
        recs.push(rec.iter().map(|s| s.to_string()).collect());
    }
    if recs.len() < MIN_ROWS + 1 {
        return None;
    }
    let counts: Vec<usize> = recs.iter().map(|r| r.len()).collect();
    let (m, c) = mode(&counts)?;
    if m < 2 || c * 20 < recs.len() * 19 {
        return None;
    }
    let mut headers = recs[0].clone();
    // `df.to_csv()` writes the index as a column with an empty name.
    if headers.first().is_some_and(|h| h.trim().is_empty())
        && !headers.iter().any(|h| h.trim() == "index")
    {
        headers[0] = "index".to_string();
    }
    // "word, word, word" is prose or a list, not a table: real column names do not start with a space.
    if headers.len() != m
        || headers
            .iter()
            .skip(1)
            .any(|h| h.starts_with(char::is_whitespace))
        || !valid_header(&headers)
    {
        return None;
    }
    let sep = (d as char).to_string();
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for r in recs.into_iter().skip(1) {
        if r.len() == m {
            rows.push(r);
        } else if notes.len() < 5 {
            notes.push(r.join(&sep));
        }
    }
    if rows.len() < MIN_ROWS {
        return None;
    }
    let format = if d == b'\t' { Format::Tsv } else { Format::Csv };
    Some(Table {
        format,
        headers,
        rows,
        notes,
    })
}

fn parse_delimited(text: &str) -> Option<Table> {
    let mut best: Option<Table> = None;
    for d in *b",\t;|" {
        if let Some(t) = try_delim(text, d) {
            let better = match &best {
                None => true,
                Some(b) => t.headers.len() > b.headers.len(),
            };
            if better {
                best = Some(t);
            }
        }
    }
    best
}

/// Splits on runs of two or more spaces (or any tab).
fn split_cols(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut run = 0usize;
    for ch in line.trim().chars() {
        if ch == ' ' {
            run += 1;
        } else if ch == '\t' {
            run += 2;
        } else {
            if run >= 2 {
                out.push(std::mem::take(&mut cur));
            } else if run == 1 {
                cur.push(' ');
            }
            run = 0;
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

/// True when some column holds, in every row, one token glued by a single space to a date
/// (`100000 2026-10-01 00:00:00`). That is how pandas can print a datetime column, and the cells
/// of the two columns can no longer be told apart, so the labels would be wrong.
fn glued_datetime_column(rows: &[Vec<String>]) -> bool {
    let after_space_is_date = |cell: &str| {
        let Some((first, rest)) = cell.split_once(' ') else {
            return false;
        };
        let b = rest.as_bytes();
        !first.is_empty()
            && b.len() >= 10
            && b[..4].iter().all(u8::is_ascii_digit)
            && b[4] == b'-'
            && b[5..7].iter().all(u8::is_ascii_digit)
            && b[7] == b'-'
            && b[8..10].iter().all(u8::is_ascii_digit)
    };
    let cols = rows.first().map_or(0, Vec::len);
    (0..cols).any(|c| {
        rows.iter()
            .all(|r| r.get(c).is_some_and(|x| after_space_is_date(x)))
    })
}

/// Whitespace-aligned tables such as `DataFrame.to_string()`.
fn parse_aligned(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !is_rule_line(l))
        .collect();
    if lines.len() < MIN_ROWS + 1 {
        return None;
    }
    let split: Vec<Vec<String>> = lines.iter().map(|l| split_cols(l)).collect();
    let counts: Vec<usize> = split.iter().skip(1).map(|s| s.len()).collect();
    let (m, c) = mode(&counts)?;
    if m < 3 || c * 10 < counts.len() * 9 {
        return None;
    }
    let h = split[0].len();
    let mut headers = split[0].clone();
    if h + 1 == m {
        headers.insert(0, "idx".to_string());
    } else if h != m || glued_datetime_column(&split[1..]) {
        // Same count as the header, but a column is glued to a datetime: with a pandas index
        // that adds up to the right count by accident, and the labels would be wrong.
        return None;
    }
    if !valid_header(&headers) {
        return None;
    }
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for (i, s) in split.iter().enumerate().skip(1) {
        if s.len() == m {
            rows.push(s.clone());
        } else {
            // A line with more cells than the rows is not a footer. It is a row, and `m` is the
            // count of the rows that lost a blank cell (pandas leaves a repeated index label
            // blank). Dropping the wide lines would drop real rows and a whole column.
            if s.len() > m {
                return None;
            }
            if notes.len() < 5 {
                notes.push(lines[i].trim().to_string());
            }
        }
    }
    if rows.len() < MIN_ROWS || (counts.len() - rows.len()) * 20 > rows.len() {
        return None;
    }
    Some(Table {
        format: Format::Aligned,
        headers,
        rows,
        notes,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn csv_text(n: usize) -> String {
        let mut s = String::from("id,name,region,amount,ok\n");
        for i in 0..n {
            s.push_str(&format!(
                "{},user_{},{},{}.5,{}\n",
                1000 + i,
                i % 7,
                ["us", "eu", "ap"][i % 3],
                i * 3,
                i % 2 == 0
            ));
        }
        s
    }

    pub fn jsonl_text(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            let level = ["info", "info", "warn", "error"][i % 4];
            s.push_str(&format!(
                "{{\"ts\":\"2026-11-02T10:{:02}:{:02}Z\",\"level\":\"{}\",\"msg\":\"event {}\",\"ms\":{}}}\n",
                i / 60,
                i % 60,
                level,
                i,
                (i * 37) % 900
            ));
        }
        s
    }

    pub fn aligned_text(n: usize) -> String {
        let mut s = String::from("   order_id                  ts   region   amount\n");
        for i in 0..n {
            s.push_str(&format!(
                "{:<3}  {}  2026-10-01 00:{:02}:00  {:>8}  {:>7}.25\n",
                i,
                100000 + i,
                i % 60,
                ["us-east", "eu-west"][i % 2],
                i * 2
            ));
        }
        s
    }

    #[test]
    fn detects_csv() {
        let t = detect(&csv_text(40)).expect("csv");
        assert_eq!(t.format, Format::Csv);
        assert_eq!(t.headers, ["id", "name", "region", "amount", "ok"]);
        assert_eq!(t.rows.len(), 40);
    }

    #[test]
    fn detects_tsv() {
        let t = detect(&csv_text(40).replace(',', "\t")).expect("tsv");
        assert_eq!(t.format, Format::Tsv);
        assert_eq!(t.rows.len(), 40);
    }

    #[test]
    fn detects_jsonl() {
        let t = detect(&jsonl_text(50)).expect("jsonl");
        assert_eq!(t.format, Format::Jsonl);
        assert_eq!(t.headers, ["ts", "level", "msg", "ms"]);
        assert_eq!(t.rows.len(), 50);
    }

    #[test]
    fn detects_json_array() {
        let objs: Vec<String> = jsonl_text(30).lines().map(String::from).collect();
        let t = detect(&format!("[{}]", objs.join(","))).expect("array");
        assert_eq!(t.format, Format::JsonArray);
        assert_eq!(t.rows.len(), 30);
    }

    #[test]
    fn detects_aligned_table_with_index() {
        let t = detect(&aligned_text(30)).expect("aligned");
        assert_eq!(t.format, Format::Aligned);
        assert_eq!(t.headers[0], "idx");
        assert_eq!(t.rows.len(), 30);
    }

    #[test]
    fn keeps_footer_lines_as_notes() {
        let mut s = jsonl_text(40);
        s.push_str("not json at all\n");
        let t = detect(&s).expect("jsonl with one bad line");
        assert_eq!(t.rows.len(), 40);
        assert_eq!(t.notes, ["not json at all"]);
    }

    #[test]
    fn rejects_prose() {
        let prose =
            "The quick brown fox jumps over the lazy dog, and then it rests, sleeps, dreams.\n"
                .repeat(30);
        assert!(detect(&prose).is_none());
    }

    #[test]
    fn rejects_source_code() {
        let code =
            "fn main() {\n    let a = foo(1, 2, 3);\n    println!(\"{}\", bar(a, b));\n}\n\n"
                .repeat(20);
        assert!(detect(&code).is_none());
    }

    #[test]
    fn rejects_single_json_object() {
        let obj = format!(
            "{{{}}}",
            (0..40)
                .map(|i| format!("\"k{i}\": {i}"))
                .collect::<Vec<_>>()
                .join(",\n")
        );
        assert!(detect(&obj).is_none());
    }

    #[test]
    fn rejects_cargo_output() {
        let out = "   Compiling foo v0.1.0\n    Finished dev [unoptimized] target(s) in 1.2s\nrunning 3 tests\ntest a ... ok\n".repeat(15);
        assert!(detect(&out).is_none());
    }

    #[test]
    fn rejects_short_input() {
        assert!(detect("a,b\n1,2\n").is_none());
    }

    #[test]
    fn numbers_with_no_header_are_a_grid_not_a_csv_with_a_numeric_header() {
        let s = "1,2,3\n".to_string() + &"4,5,6\n".repeat(40);
        let t = detect(&s).expect("a grid of numbers");
        assert_eq!(t.format, Format::Grid);
        assert_eq!(t.headers, ["c0", "c1", "c2"]);
        assert_eq!(t.rows.len(), 41);
    }

    fn wrapped_rows(n: usize) -> String {
        let rows: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    "{{\"id\": {i}, \"name\": \"user_{}\", \"amount\": {}}}",
                    i % 31,
                    i * 3
                )
            })
            .collect();
        format!("[{}]", rows.join(", "))
    }

    #[test]
    fn prose_around_json_rows_is_found_and_kept_apart() {
        let data = wrapped_rows(40);
        let text = format!("Query returned 40 rows:\n{data}\nTotal: 40\n");
        assert!(detect(&text).is_none(), "the plain reader sees prose first");
        let w = detect_wrapped(&text).unwrap();
        assert_eq!(w.table.rows.len(), 40);
        assert_eq!(&text[..w.start], "Query returned 40 rows:\n");
        assert_eq!(&text[w.start..w.end], data);
        assert_eq!(&text[w.end..], "\nTotal: 40\n");
    }

    #[test]
    fn too_much_prose_around_the_rows_is_not_a_wrapper() {
        let data = wrapped_rows(40);
        // Seven lines before the data.
        let before = "A line of text.\n".repeat(7);
        assert!(detect_wrapped(&format!("{before}{data}\n")).is_none());
        // One line that is too long to be a status line.
        let long = format!("{}\n", "word ".repeat(80));
        assert!(detect_wrapped(&format!("{long}{data}\n")).is_none());
        // Seven lines after the data.
        let after = "\nAnother line.".repeat(7);
        assert!(detect_wrapped(&format!("{data}{after}\n")).is_none());
    }

    #[test]
    fn a_line_of_json_is_not_a_header() {
        // One status line above JSON lines used to make a table whose columns were the words of
        // the first JSON line (`{"id":`, `0,`, `"name":`, ...).
        let rows: Vec<String> = (0..40)
            .map(|i| {
                format!(
                    "{{\"id\": {i}, \"name\": \"user_{}\", \"ok\": true}}",
                    i % 7
                )
            })
            .collect();
        let text = format!("Fetching page 1 of 1...\n{}\nDone.\n", rows.join("\n"));
        if let Some(t) = detect(&text) {
            assert!(
                !t.headers.iter().any(|h| h.contains('"') || h.contains('{')),
                "{:?}",
                t.headers
            );
        }
        let w = detect_wrapped(&text).unwrap();
        assert_eq!(w.table.headers, ["id", "name", "ok"]);
        assert_eq!(w.table.rows.len(), 40);
    }

    #[test]
    fn a_header_cell_that_is_a_piece_of_data_is_not_a_name() {
        let h =
            |cells: &[&str]| valid_header(&cells.iter().map(|c| c.to_string()).collect::<Vec<_>>());
        assert!(h(&["id", "name", "5m", "p95 latency", "Mounted on"]));
        assert!(!h(&["drwxr-xr-x@", "8 projectuser", "wheel"]));
        assert!(!h(&[
            "Build",
            "Run tests",
            "2026-08-04T22:01:17.57Z ##[group]Run"
        ]));
        assert!(!h(&["\u{feff}2026-08-04T22:01:17Z", "x"]));
        assert!(!h(&["layers.0.weight (128", ")"]));
        assert!(!h(&["{\"id\":", "0,"]));
    }

    #[test]
    fn a_few_stray_lines_are_counted_and_many_make_it_no_table() {
        // A Markdown table with prose lines around it. The prose is not bad rows to the reader,
        // so only the count of unaccounted lines can tell a page from a table.
        let page = |rows: usize, prose: usize| {
            let mut s = String::from("| name | value | size |\n| --- | --- | --- |\n");
            for i in 0..rows {
                s.push_str(&format!("| item{i:03} | {} | {} |\n", i * 3, i % 17));
            }
            for i in 0..prose {
                s.push_str(&format!("A sentence of the page, number {i}.\n"));
            }
            s
        };
        // 14 stray lines under 100 rows: five are kept as notes, nine are counted.
        let t = detect(&page(100, 14)).expect("a table with a few lines of text");
        assert_eq!(t.rows.len(), 100);
        assert!(
            t.notes
                .iter()
                .any(|n| n.starts_with("9 other lines are not rows")),
            "{:?}",
            t.notes
        );
        // 30 stray lines under 20 rows: a page with a table in it.
        assert!(detect(&page(20, 30)).is_none());
    }

    #[test]
    fn a_long_listing_is_left_alone() {
        let ls: String = (0..30)
            .map(|i| {
                format!(
                    "-rw-r--r--@  1 user  staff  {:>6} Oct 26 20:22 file_{i}.txt\n",
                    i * 100
                )
            })
            .collect();
        assert!(detect(&ls).is_none());
    }

    #[test]
    fn a_table_whose_rows_lost_a_blank_index_cell_is_refused() {
        // pandas leaves a repeated MultiIndex label blank, so all but the first row of a group
        // have one cell fewer. Without the line of index names the layout is not known, and
        // reading the short rows as the table would drop the group's first row and a column.
        let mut s = String::from("                  amount_sum  qty   n\n");
        for g in 0..4 {
            for p in 0..30 {
                let region = if p == 0 {
                    format!("r{g}")
                } else {
                    String::new()
                };
                s.push_str(&format!(
                    "{region:<6} sku_{p:03}  {:>10.2}  {:>3} {:>3}\n",
                    1000.0 + (g * 30 + p) as f64,
                    10 + p,
                    3 + p % 7
                ));
            }
        }
        assert!(parse_aligned(&s).is_none());
    }

    #[test]
    fn real_pandas_output_without_datetimes_is_labelled_correctly() {
        let t = detect(include_str!("../tests/fixtures/pandas_plain.txt")).expect("pandas table");
        assert_eq!(
            t.headers,
            [
                "idx",
                "order_id",
                "customer",
                "region",
                "amount",
                "latency_ms"
            ]
        );
        assert_eq!(t.rows.len(), 30);
        assert_eq!(t.rows[0][1], "100000");
        assert!(t.rows[0][2].starts_with("cust_"));
    }

    #[test]
    fn real_pandas_output_with_a_datetime_column_is_read_by_position() {
        // pandas joins a datetime column to its neighbour with one space, and the datetime holds
        // a space itself, so a split on spaces cannot tell the columns apart. The right edges of
        // the header can: every cell ends where its header ends.
        let t = detect(include_str!("../tests/fixtures/pandas_datetime.txt")).unwrap();
        assert_eq!(
            t.headers,
            [
                "idx",
                "order_id",
                "ts",
                "customer",
                "region",
                "amount",
                "latency_ms"
            ]
        );
        assert_eq!(t.rows[0][..3], ["0", "100000", "2026-10-01 00:00:00"]);
        assert_eq!(t.rows[1][2], "2026-10-01 00:07:00");
        assert!(t.rows.iter().all(|r| r.len() == t.headers.len()));
    }

    #[test]
    fn a_datetime_that_could_be_an_index_is_refused_not_guessed() {
        // Without an integer index, a datetime in the first column reads as an index and a value,
        // so the position reader refuses it. (Other readers may still take such lines as a log.)
        let mut s = String::from("                    when   n\n");
        for i in 0..30 {
            s.push_str(&format!("2026-03-01 08:30:{:02}.250 {:>3}\n", i, 100 + i));
        }
        assert!(crate::multiindex::parse_right_aligned(&s).is_none());
    }

    pub fn polars_text(n: usize) -> String {
        let mut s = String::from(
            "shape: (500, 3)\n┌──────┬───────┬────────┐\n│ id   ┆ name  ┆ amount │\n│ ---  ┆ ---   ┆ ---    │\n│ i64  ┆ str   ┆ f64    │\n╞══════╪═══════╪════════╡\n",
        );
        for i in 0..n {
            s.push_str(&format!(
                "│ {:<4} ┆ user{} ┆ {:<6} │\n",
                i,
                i % 5,
                i as f64 * 1.5
            ));
        }
        s.push_str("│ …    ┆ …     ┆ …      │\n└──────┴───────┴────────┘\n");
        s
    }

    #[test]
    fn detects_polars_tables() {
        let t = detect(&polars_text(30)).expect("polars");
        assert_eq!(t.format, Format::Bordered);
        assert_eq!(t.headers, ["id", "name", "amount"]);
        assert_eq!(
            t.rows.len(),
            30,
            "the --- row and the dtype row are not data"
        );
        assert_eq!(t.rows[0], ["0", "user0", "0"]);
        assert!(
            t.notes.iter().any(|n| n == "shape: (500, 3)"),
            "{:?}",
            t.notes
        );
        assert!(
            t.notes.iter().any(|n| n.contains("elided")),
            "{:?}",
            t.notes
        );
    }

    #[test]
    fn detects_psql_mysql_sqlite_and_markdown_tables() {
        let rows: Vec<(usize, &str, f64)> = (0..30)
            .map(|i| (i, ["alice", "bob", "carol"][i % 3], i as f64 * 2.5))
            .collect();
        let mut psql = String::from(" id |  name  | amount\n----+--------+--------\n");
        let mut mysql = String::from(
            "+----+-------+--------+\n| id | name  | amount |\n+----+-------+--------+\n",
        );
        let mut sqlite = String::from(
            "┌────┬───────┬────────┐\n│ id │ name  │ amount │\n├────┼───────┼────────┤\n",
        );
        let mut md = String::from("| id | name  | amount |\n|----|-------|--------|\n");
        for (i, n, a) in &rows {
            psql.push_str(&format!(" {:>2} | {:<6} | {:>6}\n", i, n, a));
            mysql.push_str(&format!("| {:>2} | {:<5} | {:>6} |\n", i, n, a));
            sqlite.push_str(&format!("│ {:<2} │ {:<5} │ {:<6} │\n", i, n, a));
            md.push_str(&format!("| {} | {} | {} |\n", i, n, a));
        }
        psql.push_str("(30 rows)\n");
        mysql.push_str("+----+-------+--------+\n");
        sqlite.push_str("└────┴───────┴────────┘\n");
        for (name, text) in [
            ("psql", psql),
            ("mysql", mysql),
            ("sqlite", sqlite),
            ("markdown", md),
        ] {
            let t = detect(&text).unwrap_or_else(|| panic!("{name} not detected"));
            assert_eq!(t.format, Format::Bordered, "{name}");
            assert_eq!(t.headers, ["id", "name", "amount"], "{name}");
            assert_eq!(t.rows.len(), 30, "{name}");
            assert_eq!(t.rows[1], ["1", "bob", "2.5"], "{name}");
        }
    }

    #[test]
    fn pipes_without_a_rule_line_are_not_a_table() {
        let log = "cat access.log | grep error | sort | uniq -c | head\n".repeat(30);
        assert!(detect(&log).is_none());
        let rows = "| a | b |\n".to_string() + &"| 1 | 2 |\n".repeat(30);
        assert!(
            detect(&rows).is_none(),
            "a Markdown-style table needs its rule line"
        );
    }

    #[test]
    fn ragged_bordered_text_is_not_a_table() {
        let mut s = String::from("+---+---+\n| a | b |\n+---+---+\n");
        for i in 0..40 {
            s.push_str(&format!("| {} | x | y | z |\n", i));
        }
        assert!(detect(&s).is_none());
    }
}
