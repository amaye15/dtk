//! `dtk rows | top | count | where | stats | get`: read more of the data behind a summary.
//!
//! A summary names the data file it came from. These commands answer the questions a summary
//! cannot: the rows around row 4,000, the ten largest amounts, how many rows say `failed`, one
//! part of a JSON document. They use no other program, so a summary never points the model at a
//! tool that may not be installed.
//!
//! A file is a path, or the short id that a summary prints (`dtk top 5fa6aeae75bd amount`), which
//! names a file in dtk's store.

use crate::hook::data_dir;
use dtk::lean::{analyze, parse_number};
use dtk::table::{detect, Format, Table};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The most rows one command prints.
const MAX_ROWS: usize = 200;
/// The longest cell printed, in characters. A cell may hold a whole JSON blob.
const MAX_CELL: usize = 400;
/// The most characters `dtk get` prints.
const MAX_GET: usize = 30_000;

pub const COMMANDS: [&str; 7] = ["rows", "top", "count", "group", "where", "stats", "get"];

pub const USAGE: &str = "\
dtk rows  FILE START [COUNT]          rows from START (counting from 0), COUNT default 20
dtk top   FILE COLUMN [N] [--asc]     the N rows with the largest (or smallest) COLUMN, default 10
dtk count FILE COLUMN [N]             how often each value of COLUMN occurs, default 20
dtk group FILE BY [MEASURE...]        per value of BY (a,b for two columns): the rows, and for each
                                      MEASURE its mean, sum, min and max; --agg mean,sum picks some
dtk where FILE COLUMN OP VALUE [N]    rows where COLUMN OP VALUE; OP is = != > >= < <= ~ !~
                                      (~ means contains, ignoring case); default N is 20
dtk stats FILE COLUMN...              the statistics of those columns
dtk get   FILE PATH                   one part of a JSON file: .a.b[0]  .items[]  .items | length
FILE is a path, or the short id that a summary prints.";

fn fail(msg: &str) -> i32 {
    eprintln!("dtk: {msg}");
    1
}

fn usage(msg: &str) -> i32 {
    eprintln!("dtk: {msg}\n\n{USAGE}");
    2
}

/// A path, or the id of a file in the store.
fn resolve(file: &str, json: bool) -> Result<PathBuf, String> {
    let p = Path::new(file);
    if p.exists() {
        return Ok(p.to_path_buf());
    }
    let is_id = (6..=16).contains(&file.len()) && file.bytes().all(|b| b.is_ascii_hexdigit());
    if is_id {
        if let Some(dir) = data_dir().map(|d| d.join("store")) {
            let mut hits: Vec<PathBuf> = fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.starts_with(&file.to_lowercase()))
                })
                .collect();
            hits.sort();
            // `<id>.csv` is the table; `<id>.json` and `<id>.jsonl` are raw copies. `get` wants a
            // JSON file and the others want the table, when one id names both.
            hits.sort_by_key(|p| {
                let is_csv = p.extension().and_then(|e| e.to_str()) == Some("csv");
                is_csv == json
            });
            if let Some(h) = hits.into_iter().next() {
                return Ok(h);
            }
        }
    }
    Err(format!(
        "no file {file:?} (a path, or the short id from a summary; dtk keeps its files for 14 days)"
    ))
}

fn load_table(file: &str) -> Result<Table, String> {
    let path = resolve(file, false)?;
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext == "csv" || ext == "tsv" {
        let delim = if ext == "tsv" { b'\t' } else { b',' };
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(delim)
            .flexible(true)
            .from_reader(text.as_bytes());
        let headers: Vec<String> = rdr
            .headers()
            .map_err(|e| e.to_string())?
            .iter()
            .map(String::from)
            .collect();
        let width = headers.len();
        let rows = rdr
            .records()
            .filter_map(Result::ok)
            .map(|r| {
                let mut v: Vec<String> = r.iter().map(String::from).collect();
                v.resize(width, String::new());
                v
            })
            .collect();
        return Ok(Table {
            format: if ext == "tsv" {
                Format::Tsv
            } else {
                Format::Csv
            },
            headers,
            rows,
            notes: Vec::new(),
        });
    }
    detect(&text).ok_or_else(|| format!("{} is not a table", path.display()))
}

fn column(t: &Table, name: &str) -> Result<usize, String> {
    t.headers
        .iter()
        .position(|h| h == name)
        .or_else(|| t.headers.iter().position(|h| h.eq_ignore_ascii_case(name)))
        .ok_or_else(|| {
            let shown: Vec<&str> = t.headers.iter().take(40).map(String::as_str).collect();
            format!(
                "no column {name:?}. Columns: {}{}",
                shown.join(", "),
                if t.headers.len() > 40 { ", ..." } else { "" }
            )
        })
}

fn clip(s: &str) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= MAX_CELL {
        s
    } else {
        let mut t: String = s.chars().take(MAX_CELL - 1).collect();
        t.push('…');
        t
    }
}

/// Header and rows as CSV, with a closing line that says how many were left out.
fn print_rows(t: &Table, picked: &[usize], total: usize) {
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    let _ = w.write_record(&t.headers);
    for &i in picked.iter().take(MAX_ROWS) {
        let _ = w.write_record(t.rows[i].iter().map(|c| clip(c)));
    }
    let bytes = w.into_inner().unwrap_or_default();
    print!("{}", String::from_utf8_lossy(&bytes));
    let shown = picked.len().min(MAX_ROWS);
    if total > shown {
        println!("... {} of {total} matching rows shown", shown);
    }
}

fn count_arg(args: &[String], at: usize, default: usize) -> Result<usize, String> {
    match args.get(at) {
        None => Ok(default),
        Some(s) => s
            .parse()
            .map_err(|_| format!("{s:?} is not a whole number")),
    }
}

fn rows_cmd(args: &[String]) -> i32 {
    let (Some(file), Some(start)) = (args.first(), args.get(1)) else {
        return usage("dtk rows needs FILE and START");
    };
    let (Ok(start), Ok(count)) = (
        start.parse::<usize>().map_err(|_| ()),
        count_arg(args, 2, 20).map_err(|_| ()),
    ) else {
        return usage("START and COUNT must be whole numbers");
    };
    let t = match load_table(file) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let end = (start + count).min(t.rows.len());
    let picked: Vec<usize> = (start.min(t.rows.len())..end).collect();
    print_rows(&t, &picked, picked.len());
    if end < t.rows.len() {
        println!("... {} more rows after row {}", t.rows.len() - end, end - 1);
    }
    0
}

fn top_cmd(args: &[String]) -> i32 {
    let (Some(file), Some(col)) = (args.first(), args.get(1)) else {
        return usage("dtk top needs FILE and COLUMN");
    };
    let asc = args.iter().any(|a| a == "--asc");
    let rest: Vec<String> = args.iter().filter(|a| *a != "--asc").cloned().collect();
    let Ok(n) = count_arg(&rest, 2, 10) else {
        return usage("N must be a whole number");
    };
    let t = match load_table(file) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let c = match column(&t, col) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let mut vals: Vec<(usize, f64)> = t
        .rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| parse_number(&r[c]).map(|(f, _)| (i, f)))
        .collect();
    if vals.is_empty() {
        return fail(&format!("column {:?} has no numbers", t.headers[c]));
    }
    vals.sort_by(|a, b| {
        let o = a.1.total_cmp(&b.1);
        (if asc { o } else { o.reverse() }).then(a.0.cmp(&b.0))
    });
    let picked: Vec<usize> = vals.iter().take(n.min(MAX_ROWS)).map(|v| v.0).collect();
    print_rows(&t, &picked, picked.len());
    let skipped = t.rows.len() - vals.len();
    if skipped > 0 {
        println!(
            "... {skipped} rows have no number in {:?} and are left out",
            t.headers[c]
        );
    }
    0
}

fn count_cmd(args: &[String]) -> i32 {
    let (Some(file), Some(col)) = (args.first(), args.get(1)) else {
        return usage("dtk count needs FILE and COLUMN");
    };
    let Ok(n) = count_arg(args, 2, 20) else {
        return usage("N must be a whole number");
    };
    let t = match load_table(file) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let c = match column(&t, col) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for r in &t.rows {
        *counts.entry(r[c].as_str()).or_default() += 1;
    }
    let mut v: Vec<(&str, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    let _ = w.write_record([t.headers[c].as_str(), "count"]);
    for (val, k) in v.iter().take(n.min(MAX_ROWS)) {
        let _ = w.write_record([clip(val).as_str(), k.to_string().as_str()]);
    }
    print!(
        "{}",
        String::from_utf8_lossy(&w.into_inner().unwrap_or_default())
    );
    if v.len() > n {
        println!("... {} more distinct values", v.len() - n);
    }
    0
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    NotContains,
}

fn parse_op(s: &str) -> Option<Op> {
    Some(match s {
        "=" | "==" => Op::Eq,
        "!=" => Op::Ne,
        ">" => Op::Gt,
        ">=" => Op::Ge,
        "<" => Op::Lt,
        "<=" => Op::Le,
        "~" => Op::Contains,
        "!~" => Op::NotContains,
        _ => return None,
    })
}

/// Does a cell pass? Numbers compare as numbers (`$1,200` and `1200` are equal); anything else
/// compares as text.
fn passes(cell: &str, op: Op, value: &str) -> bool {
    match op {
        Op::Contains => return cell.to_lowercase().contains(&value.to_lowercase()),
        Op::NotContains => return !cell.to_lowercase().contains(&value.to_lowercase()),
        _ => {}
    }
    let ordering_op = matches!(op, Op::Gt | Op::Ge | Op::Lt | Op::Le);
    let ord = match (parse_number(cell), parse_number(value)) {
        (Some((a, _)), Some((b, _))) => a.partial_cmp(&b),
        // `> 5` on a cell that is not a number (N/A, a word) is not true, whatever the letters.
        (None, Some(_)) if ordering_op => None,
        _ => Some(cell.cmp(value)),
    };
    let Some(ord) = ord else { return false };
    match op {
        Op::Eq => ord.is_eq(),
        Op::Ne => ord.is_ne(),
        Op::Gt => ord.is_gt(),
        Op::Ge => ord.is_ge(),
        Op::Lt => ord.is_lt(),
        Op::Le => ord.is_le(),
        Op::Contains | Op::NotContains => unreachable!(),
    }
}

fn where_cmd(args: &[String]) -> i32 {
    let (Some(file), Some(col), Some(op), Some(value)) =
        (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return usage("dtk where needs FILE COLUMN OP VALUE");
    };
    let Some(op) = parse_op(op) else {
        return usage(&format!("{op:?} is not an operator"));
    };
    let Ok(n) = count_arg(args, 4, 20) else {
        return usage("N must be a whole number");
    };
    let t = match load_table(file) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let c = match column(&t, col) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let matching: Vec<usize> = (0..t.rows.len())
        .filter(|&i| passes(&t.rows[i][c], op, value))
        .collect();
    let picked: Vec<usize> = matching.iter().copied().take(n.min(MAX_ROWS)).collect();
    print_rows(&t, &picked, matching.len());
    println!("{} of {} rows match", matching.len(), t.rows.len());
    0
}

/// A number as `dtk group` prints it: whole numbers whole, others to four places without trailing
/// zeros.
fn plain(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let s = format!("{x:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn group_cmd(args: &[String]) -> i32 {
    let (Some(file), Some(by)) = (args.first(), args.get(1)) else {
        return usage("dtk group needs FILE and BY");
    };
    // `--agg mean,sum` and `--limit N` can come anywhere after BY.
    let mut aggs: Vec<&str> = vec!["mean", "sum", "min", "max"];
    let mut limit = 50usize;
    let mut measures: Vec<&String> = Vec::new();
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--agg" => match it.next() {
                Some(list) => {
                    aggs = list.split(',').map(str::trim).collect();
                    if let Some(bad) = aggs
                        .iter()
                        .find(|x| !["mean", "sum", "min", "max"].contains(x))
                    {
                        return usage(&format!("{bad:?} is not mean, sum, min or max"));
                    }
                }
                None => return usage("--agg needs a list such as mean,sum"),
            },
            "--limit" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => limit = n,
                None => return usage("--limit needs a whole number"),
            },
            _ => measures.push(a),
        }
    }
    let t = match load_table(file) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let by_cols: Result<Vec<usize>, String> = by.split(',').map(|b| column(&t, b.trim())).collect();
    let by_cols = match by_cols {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let mut mcols = Vec::new();
    for m in &measures {
        match column(&t, m) {
            Ok(c) => mcols.push(c),
            Err(e) => return fail(&e),
        }
    }
    // One entry per group: the rows in it, and for each measure the numbers seen.
    struct Acc {
        rows: usize,
        n: Vec<usize>,
        sum: Vec<f64>,
        min: Vec<f64>,
        max: Vec<f64>,
    }
    let mut groups: std::collections::HashMap<Vec<&str>, Acc> = std::collections::HashMap::new();
    let mut order: Vec<Vec<&str>> = Vec::new();
    for r in &t.rows {
        let key: Vec<&str> = by_cols.iter().map(|&c| r[c].as_str()).collect();
        let acc = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Acc {
                rows: 0,
                n: vec![0; mcols.len()],
                sum: vec![0.0; mcols.len()],
                min: vec![f64::INFINITY; mcols.len()],
                max: vec![f64::NEG_INFINITY; mcols.len()],
            }
        });
        acc.rows += 1;
        for (i, &c) in mcols.iter().enumerate() {
            if let Some((v, _)) = parse_number(&r[c]) {
                acc.n[i] += 1;
                acc.sum[i] += v;
                acc.min[i] = acc.min[i].min(v);
                acc.max[i] = acc.max[i].max(v);
            }
        }
    }
    order.sort_by(|a, b| groups[b].rows.cmp(&groups[a].rows).then(a.cmp(b)));
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    let mut head: Vec<String> = by_cols.iter().map(|&c| t.headers[c].clone()).collect();
    head.push("rows".into());
    for &c in &mcols {
        for a in &aggs {
            head.push(format!("{}_{a}", t.headers[c]));
        }
    }
    let _ = w.write_record(&head);
    for key in order.iter().take(limit.min(MAX_ROWS)) {
        let acc = &groups[key];
        let mut rec: Vec<String> = key.iter().map(|k| clip(k)).collect();
        rec.push(acc.rows.to_string());
        for i in 0..mcols.len() {
            for a in &aggs {
                rec.push(if acc.n[i] == 0 {
                    String::new()
                } else {
                    plain(match *a {
                        "mean" => acc.sum[i] / acc.n[i] as f64,
                        "sum" => acc.sum[i],
                        "min" => acc.min[i],
                        _ => acc.max[i],
                    })
                });
            }
        }
        let _ = w.write_record(&rec);
    }
    print!(
        "{}",
        String::from_utf8_lossy(&w.into_inner().unwrap_or_default())
    );
    if order.len() > limit.min(MAX_ROWS) {
        println!("... {} more groups", order.len() - limit.min(MAX_ROWS));
    }
    0
}

fn stats_cmd(args: &[String]) -> i32 {
    let Some(file) = args.first() else {
        return usage("dtk stats needs FILE and at least one COLUMN");
    };
    if args.len() < 2 {
        return usage("dtk stats needs at least one COLUMN");
    }
    let t = match load_table(file) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let info = analyze(&t);
    let mut status = 0;
    for name in &args[1..] {
        match column(&t, name) {
            Ok(c) => println!("{}: {}", t.headers[c], info[c].summary),
            Err(e) => status = fail(&e),
        }
    }
    status
}

// ---- dtk get ----------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Step {
    Key(String),
    Index(i64),
    Each,
}

/// `.a.b[0]`, `.items[]`, `.["odd key"]`, `[2]`. The leading dot is optional.
fn parse_path(p: &str) -> Result<Vec<Step>, String> {
    let c: Vec<char> = p.trim().chars().collect();
    let mut i = 0;
    let mut steps = Vec::new();
    while i < c.len() {
        match c[i] {
            '.' => {
                i += 1;
                let start = i;
                while i < c.len() && (c[i].is_alphanumeric() || matches!(c[i], '_' | '-')) {
                    i += 1;
                }
                if i > start {
                    steps.push(Step::Key(c[start..i].iter().collect()));
                }
            }
            '[' => {
                let close = c[i..]
                    .iter()
                    .position(|x| *x == ']')
                    .ok_or("a [ without a ]")?
                    + i;
                let inner: String = c[i + 1..close].iter().collect();
                let inner = inner.trim();
                steps.push(if inner.is_empty() {
                    Step::Each
                } else if let Ok(n) = inner.parse::<i64>() {
                    Step::Index(n)
                } else if inner.len() >= 2 && inner.starts_with('"') && inner.ends_with('"') {
                    Step::Key(inner[1..inner.len() - 1].to_string())
                } else {
                    return Err(format!("cannot read [{inner}]"));
                });
                i = close + 1;
            }
            x if x.is_alphanumeric() || x == '_' => {
                // a bare first key: items[0]
                let start = i;
                while i < c.len() && (c[i].is_alphanumeric() || matches!(c[i], '_' | '-')) {
                    i += 1;
                }
                steps.push(Step::Key(c[start..i].iter().collect()));
            }
            x => return Err(format!("unexpected {x:?} in the path")),
        }
    }
    Ok(steps)
}

fn apply(values: Vec<Value>, step: &Step) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for v in values {
        match (step, v) {
            (Step::Key(k), Value::Object(m)) => {
                out.push(m.get(k).cloned().unwrap_or(Value::Null));
            }
            (Step::Key(k), other) => {
                return Err(format!("cannot take key {k:?} of {}", kind_of(&other)))
            }
            (Step::Index(n), Value::Array(a)) => {
                let i = if *n < 0 { a.len() as i64 + n } else { *n };
                out.push(
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| a.get(i))
                        .cloned()
                        .unwrap_or(Value::Null),
                );
            }
            (Step::Index(_), other) => return Err(format!("cannot index {}", kind_of(&other))),
            (Step::Each, Value::Array(a)) => out.extend(a),
            (Step::Each, Value::Object(m)) => out.extend(m.into_iter().map(|(_, v)| v)),
            (Step::Each, other) => return Err(format!("cannot iterate over {}", kind_of(&other))),
        }
    }
    Ok(out)
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn load_json(file: &str) -> Result<Value, String> {
    let path = resolve(file, true)?;
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Ok(v);
    }
    // JSON lines: an array of the lines
    let lines: Result<Vec<Value>, _> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str::<Value>)
        .collect();
    lines
        .map(Value::Array)
        .map_err(|_| format!("{} is not JSON", path.display()))
}

fn get_cmd(args: &[String]) -> i32 {
    let (Some(file), Some(expr)) = (args.first(), args.get(1)) else {
        return usage("dtk get needs FILE and PATH");
    };
    // `.items | length` and `.items | keys`
    let (path, tail) = match expr.split_once('|') {
        Some((p, t)) => (p, Some(t.trim())),
        None => (expr.as_str(), None),
    };
    let steps = match parse_path(path) {
        Ok(s) => s,
        Err(e) => return usage(&e),
    };
    if !matches!(tail, None | Some("length") | Some("keys")) {
        return usage("after | only `length` and `keys` are known");
    }
    let doc = match load_json(file) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let mut values = vec![doc];
    for s in &steps {
        values = match apply(values, s) {
            Ok(v) => v,
            Err(e) => return fail(&e),
        };
    }
    let mut printed = 0usize;
    for v in values {
        let v = match tail {
            Some("length") => match &v {
                Value::Array(a) => Value::from(a.len()),
                Value::Object(m) => Value::from(m.len()),
                Value::String(s) => Value::from(s.chars().count()),
                Value::Null => Value::from(0),
                other => return fail(&format!("{} has no length", kind_of(other))),
            },
            Some("keys") => match &v {
                Value::Object(m) => Value::Array(m.keys().cloned().map(Value::from).collect()),
                other => return fail(&format!("{} has no keys", kind_of(other))),
            },
            _ => v,
        };
        let line = serde_json::to_string(&v).unwrap_or_default();
        if printed + line.len() > MAX_GET {
            println!("... output cut at {MAX_GET} characters; ask for a smaller part");
            return 0;
        }
        printed += line.len() + 1;
        println!("{line}");
    }
    0
}

/// Runs one of `COMMANDS`.
pub fn run(cmd: &str, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    match cmd {
        "rows" => rows_cmd(args),
        "top" => top_cmd(args),
        "count" => count_cmd(args),
        "group" => group_cmd(args),
        "where" => where_cmd(args),
        "stats" => stats_cmd(args),
        "get" => get_cmd(args),
        _ => usage(&format!("unknown command {cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_paths() {
        assert_eq!(
            parse_path(".a.b[0]").unwrap(),
            [Step::Key("a".into()), Step::Key("b".into()), Step::Index(0)]
        );
        assert_eq!(
            parse_path("items[]").unwrap(),
            [Step::Key("items".into()), Step::Each]
        );
        assert_eq!(
            parse_path(".[\"odd key\"][-1]").unwrap(),
            [Step::Key("odd key".into()), Step::Index(-1)]
        );
        assert_eq!(parse_path(".").unwrap(), Vec::<Step>::new());
        assert!(parse_path(".a[").is_err());
        assert!(parse_path(".a[x]").is_err());
        assert!(parse_path(".a|b").is_err());
    }

    #[test]
    fn compares_numbers_as_numbers_and_the_rest_as_text() {
        assert!(passes("$1,200", Op::Eq, "1200"));
        assert!(passes("9", Op::Lt, "10"), "9 < 10 as numbers, not as text");
        assert!(passes("abc", Op::Lt, "abd"));
        assert!(passes("Failed: disk", Op::Contains, "failed"));
        assert!(passes("ok", Op::NotContains, "fail"));
        assert!(
            !passes("N/A", Op::Gt, "5"),
            "text and a number compare as text"
        );
    }
}
