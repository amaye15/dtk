//! Table detection: turns raw command output into a header plus rows, or says "not a table".
//!
//! Every parser is strict on purpose. A false positive replaces real output with a summary,
//! so unsure input must return `None`.

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// Fewest data rows that count as a table.
pub const MIN_ROWS: usize = 10;
/// Most columns a table may have.
const MAX_COLS: usize = 80;
/// Largest input the parsers accept.
pub const MAX_INPUT: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jsonl,
    JsonArray,
    Csv,
    Tsv,
    Aligned,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Jsonl => "jsonl",
            Format::JsonArray => "json-array",
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Aligned => "table",
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
    if text.len() > MAX_INPUT {
        return None;
    }
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.len() < 200 {
        return None;
    }
    match text.as_bytes()[0] {
        b'[' => {
            if let Some(t) = parse_json_array(text) {
                return Some(t);
            }
        }
        b'{' => return parse_jsonl(text),
        _ => {}
    }
    parse_delimited(text).or_else(|| parse_aligned(text))
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Builds a table from JSON objects. Columns are the union of keys in first-seen order.
fn from_objects<'a>(
    format: Format,
    objects: impl Iterator<Item = &'a Map<String, Value>>,
    notes: Vec<String>,
) -> Option<Table> {
    let mut headers: Vec<String> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut sparse: Vec<Vec<(usize, String)>> = Vec::new();
    for obj in objects {
        let mut row = Vec::with_capacity(obj.len());
        for (k, v) in obj {
            let i = match index.get(k.as_str()) {
                Some(i) => *i,
                None => {
                    if headers.len() >= MAX_COLS {
                        return None;
                    }
                    headers.push(k.clone());
                    index.insert(k.as_str(), headers.len() - 1);
                    headers.len() - 1
                }
            };
            row.push((i, cell(v)));
        }
        sparse.push(row);
    }
    if sparse.len() < MIN_ROWS || headers.len() < 2 {
        return None;
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
    Some(Table {
        format,
        headers,
        rows,
        notes,
    })
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

fn valid_header(h: &[String]) -> bool {
    if h.len() < 2 || h.len() > MAX_COLS {
        return false;
    }
    let mut seen = HashSet::new();
    for c in h {
        let c = c.trim();
        if c.is_empty()
            || c.chars().count() > 60
            || c.split_whitespace().count() > 6
            || c.parse::<f64>().is_ok()
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
fn mode(counts: &[usize]) -> Option<(usize, usize)> {
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
    let headers = recs[0].clone();
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

/// Whitespace-aligned tables such as `DataFrame.to_string()`.
fn parse_aligned(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
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
    } else if h != m {
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
        } else if notes.len() < 5 {
            notes.push(lines[i].trim().to_string());
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
    fn rejects_numeric_header() {
        let s = "1,2,3\n".to_string() + &"4,5,6\n".repeat(40);
        assert!(detect(&s).is_none());
    }
}
