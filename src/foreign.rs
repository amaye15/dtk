//! Data written in ways other than the usual table formats:
//!
//! - **Python repr**: `print(rows)` of a list of dicts, or `df.to_dict("records")`. Single quotes,
//!   `True`, `None`, `nan`, `Timestamp('...')`. Not JSON, but the same data.
//! - **Columnar JSON**: `df.to_json(orient="split")` (`{"columns": [...], "data": [[...]]}`) and
//!   an array of arrays whose first row names the columns.
//! - **Number grids**: `print(np.array(...))`, `np.savetxt`, a matrix: rows of numbers, no header.
//! - **Headerless delimited text**: `psql -t -A`, `sqlite3 -list`, `cut -d,`.
//!
//! Each parser is strict. Headerless data has no names to check, so the evidence is in the cells:
//! the same number of fields on (almost) every line, and columns that are numbers.

use crate::lean::parse_number;
use crate::table::{valid_header, Format, Table, MIN_ROWS};
use serde_json::{Map, Number, Value};

const MAX_DEPTH: usize = 32;

// ---- Python repr -------------------------------------------------------------------------------

struct Py<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl<'a> Py<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> Option<String> {
        let q = *self.s.get(self.i)?;
        if q != b'\'' && q != b'"' {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let rest = self.text.get(self.i..)?;
            let c = rest.chars().next()?;
            self.i += c.len_utf8();
            match c {
                c if c as u32 == q as u32 => return Some(out),
                '\\' => {
                    let e = self.text.get(self.i..)?.chars().next()?;
                    self.i += e.len_utf8();
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'x' => {
                            let h = self.text.get(self.i..self.i + 2)?;
                            out.push(char::from_u32(u32::from_str_radix(h, 16).ok()?)?);
                            self.i += 2;
                        }
                        'u' => {
                            let h = self.text.get(self.i..self.i + 4)?;
                            out.push(char::from_u32(u32::from_str_radix(h, 16).ok()?)?);
                            self.i += 4;
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }

    fn ident(&mut self) -> Option<&'a str> {
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'_' | b'.'))
        {
            self.i += 1;
        }
        (self.i > start).then(|| &self.text[start..self.i])
    }

    /// The source text of `name(...)`, from `(` to its matching `)`.
    fn call_args(&mut self) -> Option<&'a str> {
        let start = self.i;
        let mut depth = 0usize;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return Some(&self.text[start + 1..self.i - 1]);
                    }
                }
                q @ (b'\'' | b'"') => {
                    self.i += 1;
                    while self.i < self.s.len() && self.s[self.i] != q {
                        if self.s[self.i] == b'\\' {
                            self.i += 1;
                        }
                        self.i += 1;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        None
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.s.get(self.i)? {
            b'[' | b'(' => {
                let close = if self.s[self.i] == b'[' { b']' } else { b')' };
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    if self.eat(close) {
                        return Some(Value::Array(items));
                    }
                    items.push(self.value(depth + 1)?);
                    if !self.eat(b',') {
                        return self.eat(close).then_some(Value::Array(items));
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut m = Map::new();
                loop {
                    if self.eat(b'}') {
                        return Some(Value::Object(m));
                    }
                    self.ws();
                    let key = match self.value(depth + 1)? {
                        Value::String(s) => s,
                        Value::Number(n) => n.to_string(),
                        _ => return None,
                    };
                    if !self.eat(b':') {
                        return None;
                    }
                    let v = self.value(depth + 1)?;
                    m.insert(key, v);
                    if !self.eat(b',') {
                        return self.eat(b'}').then_some(Value::Object(m));
                    }
                }
            }
            b'\'' | b'"' => self.string().map(Value::String),
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_digit()
                        || matches!(self.s[self.i], b'.' | b'e' | b'E' | b'+' | b'-' | b'_'))
                {
                    self.i += 1;
                }
                let t = self.text[start..self.i].replace('_', "");
                if t == "-" {
                    // -inf
                    let w = self.ident()?;
                    return (w == "inf").then(|| Value::String("-inf".into()));
                }
                match t.parse::<i64>() {
                    Ok(n) => Some(Value::from(n)),
                    Err(_) => Number::from_f64(t.parse::<f64>().ok()?).map(Value::Number),
                }
            }
            _ => {
                let name = self.ident()?;
                self.ws();
                if self.s.get(self.i) == Some(&b'(') {
                    let args = self.call_args()?;
                    return Some(simplify_call(name, args));
                }
                match name {
                    "True" => Some(Value::Bool(true)),
                    "False" => Some(Value::Bool(false)),
                    "None" | "nan" | "NaN" | "NaT" | "pd.NaT" | "np.nan" => Some(Value::Null),
                    "inf" => Some(Value::String("inf".into())),
                    _ => None,
                }
            }
        }
    }
}

/// `Timestamp('2026-01-01 00:00:00')` is its text, `np.float64(1.5)` is the number, and any other
/// call stays as the source text, so that nothing is lost.
fn simplify_call(name: &str, args: &str) -> Value {
    let base = name.rsplit('.').next().unwrap_or(name);
    let inner = args.trim();
    let unquoted = |s: &str| -> Option<String> {
        let mut p = Py {
            s: s.as_bytes(),
            text: s,
            i: 0,
        };
        p.string()
    };
    match base {
        "Timestamp" | "Decimal" | "Timedelta" | "Period" => {
            if let Some(s) = unquoted(inner.split(',').next().unwrap_or("")) {
                return Value::String(s);
            }
        }
        "int64" | "int32" | "int16" | "int8" | "uint8" | "uint16" | "uint32" | "uint64"
        | "float64" | "float32" | "float16" | "bool_" | "bool" | "int" | "float" => {
            let mut p = Py {
                s: inner.as_bytes(),
                text: inner,
                i: 0,
            };
            if let Some(v) = p.value(0) {
                return v;
            }
        }
        _ => {}
    }
    Value::String(format!("{name}({args})"))
}

fn records_from_value(v: Value) -> Option<Vec<Map<String, Value>>> {
    match v {
        Value::Array(a) => {
            let n = a.len();
            let objs: Vec<Map<String, Value>> = a
                .into_iter()
                .filter_map(|x| match x {
                    Value::Object(m) => Some(m),
                    _ => None,
                })
                .collect();
            (objs.len() * 20 >= n * 19).then_some(objs)
        }
        // {'a': [1, 2, 3], 'b': [4, 5, 6]}: one list per column (`to_dict("list")`)
        Value::Object(m) => {
            let cols: Vec<(&String, &Vec<Value>)> = m
                .iter()
                .filter_map(|(k, v)| v.as_array().map(|a| (k, a)))
                .collect();
            let len = cols.first()?.1.len();
            if cols.len() != m.len() || cols.len() < 2 || cols.iter().any(|c| c.1.len() != len) {
                return None;
            }
            Some(
                (0..len)
                    .map(|i| {
                        cols.iter()
                            .map(|(k, a)| ((*k).clone(), a[i].clone()))
                            .collect()
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// A Python list of dicts (or a dict of lists), as `print` shows it. Also the other ways pandas
/// prints a frame: `to_dict()` (a dict per column), `to_dict("index")`, `to_dict("split")`,
/// `values.tolist()` (a list of lists), and one dict per line.
pub fn parse_python_repr(text: &str) -> Option<Table> {
    let t = text.trim();
    let b = t.as_bytes();
    // Only text that is a list or dict of container things, and not JSON (which parsed earlier).
    let list_of_containers = b.first() == Some(&b'[') && matches!(b.get(1), Some(b'{' | b'['));
    if !(list_of_containers || b.first() == Some(&b'{')) {
        return None;
    }
    let mut p = Py {
        s: b,
        text: t,
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.i != b.len() {
        return python_dict_per_line(t);
    }
    // Valid JSON is not Python repr: the JSON parsers decide what it is (a wrapper with two
    // big arrays is ambiguous, and stays so).
    if serde_json::from_str::<Value>(t).is_ok() {
        return None;
    }
    nested_table(Format::PyRepr, v)
}

/// One dict per line (`for r in rows: print(r)`).
fn python_dict_per_line(text: &str) -> Option<Table> {
    let mut objects: Vec<Map<String, Value>> = Vec::new();
    let mut lines = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        let l = line.trim();
        let mut p = Py {
            s: l.as_bytes(),
            text: l,
            i: 0,
        };
        if let Some(Value::Object(m)) = p.value(0) {
            p.ws();
            if p.i == l.len() {
                objects.push(m);
            }
        }
    }
    if objects.len() < MIN_ROWS || objects.len() * 20 < lines * 19 {
        return None;
    }
    crate::table::from_object_list(Format::PyRepr, &objects)
}

fn scalar(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

/// The ways pandas prints a frame as one nested value, from the most to the least specific.
fn nested_table(format: Format, v: Value) -> Option<Table> {
    let retag = |mut t: Table| {
        t.format = format;
        t
    };
    if let Some(t) = parse_columnar(&v) {
        return Some(retag(t));
    }
    if let Some(t) = column_dicts(format, &v) {
        return Some(t);
    }
    if let Some(t) = crate::table::from_keyed_root(format, &v) {
        return Some(t);
    }
    if let Some(t) = list_of_lists(format, &v) {
        return Some(t);
    }
    let objects = records_from_value(v)?;
    crate::table::from_object_list(format, &objects)
}

/// The shapes that need a Python or JSON value, for JSON text that is not a list of records:
/// `df.to_json()` and a list of lists.
pub fn parse_nested_json(root: &Value) -> Option<Table> {
    match root {
        Value::Object(_) => column_dicts(Format::JsonObject, root),
        Value::Array(_) => list_of_lists(Format::JsonArray, root),
        _ => None,
    }
}

/// `{'a': {0: 1, 1: 2}, 'b': {0: 'x', 1: 'y'}}`: one dict per column, keyed by the row label
/// (`df.to_dict()` and `df.to_json()`, which are what pandas gives by default). The row labels
/// become a first column. A frame has more rows than columns, which tells this from rows keyed by
/// id (`{"a1": {...}, "a2": {...}}`); when the two are close, the other reading is kept.
fn column_dicts(format: Format, root: &Value) -> Option<Table> {
    let m = root.as_object()?;
    let cols: Vec<(&String, &Map<String, Value>)> = m
        .iter()
        .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
        .collect();
    if cols.len() != m.len() || cols.len() < 2 {
        return None;
    }
    let n = cols[0].1.len();
    if n < MIN_ROWS || n < cols.len() * 2 {
        return None;
    }
    let labels: Vec<&String> = cols[0].1.keys().collect();
    if cols
        .iter()
        .any(|(_, o)| o.len() != n || !labels.iter().all(|k| o.contains_key(k.as_str())))
    {
        return None;
    }
    let first = if m.contains_key("index") {
        "row"
    } else {
        "index"
    };
    let mut headers = vec![first.to_string()];
    headers.extend(cols.iter().map(|(k, _)| (*k).clone()));
    if !valid_header(&headers) {
        return None;
    }
    let rows: Vec<Vec<String>> = labels
        .iter()
        .map(|l| {
            let mut r = vec![(*l).clone()];
            r.extend(
                cols.iter()
                    .map(|(_, o)| o.get(l.as_str()).map_or_else(String::new, cell)),
            );
            r
        })
        .collect();
    Some(Table {
        format,
        headers,
        rows,
        notes: Vec::new(),
    })
}

/// `[[1, 'a', 2.5], [2, 'b', 3.5], ...]`: rows with no names (`df.values.tolist()`). The columns
/// are called c0, c1, ... and one of them must hold numbers, or it is a list of words.
fn list_of_lists(format: Format, root: &Value) -> Option<Table> {
    let a = root.as_array()?;
    if a.len() < MIN_ROWS {
        return None;
    }
    let w = a.first()?.as_array()?.len();
    if !(2..=150).contains(&w) {
        return None;
    }
    let rows: Vec<Vec<String>> = a
        .iter()
        .filter_map(Value::as_array)
        .filter(|r| r.len() == w && r.iter().all(scalar))
        .map(|r| r.iter().map(cell).collect())
        .collect();
    if rows.len() * 20 < a.len() * 19 {
        return None;
    }
    let numeric = (0..w).any(|c| {
        rows.iter().filter(|r| r[c].parse::<f64>().is_ok()).count() * 10 >= rows.len() * 9
    });
    if !numeric {
        return None;
    }
    Some(Table {
        format,
        headers: generic_headers(w),
        rows,
        notes: Vec::new(),
    })
}

// ---- columnar JSON -----------------------------------------------------------------------------

fn column_names(v: &Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|c| match c {
            Value::String(s) => Some(s.clone()),
            Value::Object(m) => ["name", "field", "label", "title"]
                .iter()
                .find_map(|k| m.get(*k).and_then(Value::as_str).map(String::from)),
            _ => None,
        })
        .collect()
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn table_of_arrays(format: Format, headers: Vec<String>, data: &[Value]) -> Option<Table> {
    let w = headers.len();
    let rows: Vec<Vec<String>> = data
        .iter()
        .filter_map(Value::as_array)
        .filter(|r| r.len() == w)
        .map(|r| r.iter().map(cell).collect())
        .collect();
    if rows.len() < MIN_ROWS || rows.len() * 20 < data.len() * 19 || !valid_header(&headers) {
        return None;
    }
    Some(Table {
        format,
        headers,
        rows,
        notes: Vec::new(),
    })
}

/// `{"columns": [...], "data": [[...], ...]}` (pandas `orient="split"`, DuckDB, Spark),
/// `{"headers": [...], "rows": [[...]]}`, or an array of arrays whose first row is the header.
pub fn parse_columnar(root: &Value) -> Option<Table> {
    match root {
        Value::Object(m) => {
            let names = ["columns", "headers", "header", "fields"]
                .iter()
                .find_map(|k| m.get(*k).and_then(column_names))?;
            let data = ["data", "rows", "values", "records"]
                .iter()
                .find_map(|k| m.get(*k).and_then(Value::as_array))?;
            table_of_arrays(Format::JsonObject, names, data)
        }
        Value::Array(a) if a.len() > MIN_ROWS => {
            let header: Vec<String> = a
                .first()?
                .as_array()?
                .iter()
                .map(|c| c.as_str().map(String::from))
                .collect::<Option<_>>()?;
            // A list of lists of words is not a table: some column must hold numbers.
            let body = &a[1..];
            let numeric_col = (0..header.len()).any(|c| {
                let ok = body
                    .iter()
                    .filter(|r| r.get(c).is_some_and(|x| x.is_number()))
                    .count();
                ok * 10 >= body.len() * 9
            });
            if !numeric_col {
                return None;
            }
            table_of_arrays(Format::JsonArray, header, body)
        }
        _ => None,
    }
}

// ---- number grids and headerless text ----------------------------------------------------------

fn numeric_tokens(line: &str) -> Option<Vec<f64>> {
    let cleaned: String = line
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | ',' | ';' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let toks: Vec<&str> = cleaned.split_whitespace().collect();
    if toks.is_empty() {
        return Some(Vec::new());
    }
    toks.iter()
        .map(|t| t.parse::<f64>().ok().filter(|f| f.is_finite()))
        .collect()
}

fn generic_headers(w: usize) -> Vec<String> {
    (0..w).map(|i| format!("c{i}")).collect()
}

/// Rows of numbers with no header: a printed matrix, `np.savetxt`, a table of measurements.
pub fn parse_number_grid(text: &str) -> Option<Table> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut other = 0usize;
    let mut notes = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match numeric_tokens(line) {
            Some(v) if v.len() >= 2 => rows.push(
                line.chars()
                    .map(|c| {
                        if matches!(c, '[' | ']' | ',' | ';' | '\t') {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect::<String>()
                    .split_whitespace()
                    .map(String::from)
                    .collect(),
            ),
            Some(v) if v.is_empty() => {}
            _ => {
                other += 1;
                if notes.len() < 5 {
                    notes.push(line.trim().to_string());
                }
            }
        }
    }
    let widths: Vec<usize> = rows.iter().map(Vec::len).collect();
    let (w, same) = crate::table::mode(&widths)?;
    if rows.len() < MIN_ROWS
        || !(2..=200).contains(&w)
        || same * 20 < rows.len() * 19
        || other * 20 > rows.len()
    {
        return None;
    }
    rows.retain(|r| r.len() == w);
    Some(Table {
        format: Format::Grid,
        headers: generic_headers(w),
        rows,
        notes,
    })
}

/// Delimited text with no header row. There are no names to check, so the cells must show it is
/// data: the same number of fields on 98% of the lines, three or more fields, at least a third of
/// the columns numbers, and short cells.
pub fn parse_headerless(text: &str) -> Option<Table> {
    for d in *b"|,\t;" {
        if let Some(t) = headerless_with(text, d) {
            return Some(t);
        }
    }
    None
}

fn headerless_with(text: &str, d: u8) -> Option<Table> {
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
        recs.push(r.ok()?.iter().map(String::from).collect());
    }
    if recs.len() < MIN_ROWS * 2 {
        return None;
    }
    let widths: Vec<usize> = recs.iter().map(Vec::len).collect();
    let (w, same) = crate::table::mode(&widths)?;
    if !(3..=200).contains(&w) || same * 50 < recs.len() * 49 {
        return None;
    }
    recs.retain(|r| r.len() == w);
    // `| 1 | 2 |`: a field before the first delimiter and after the last, empty on every line, is a
    // drawn table (Markdown, psql), whether or not it has its rule line.
    if recs
        .iter()
        .all(|r| r[0].trim().is_empty() && r[w - 1].trim().is_empty())
    {
        return None;
    }
    if recs.iter().flatten().any(|c| c.chars().count() > 60) {
        return None;
    }
    let numeric_cols = (0..w)
        .filter(|&c| {
            let n = recs
                .iter()
                .filter(|r| parse_number(&r[c]).is_some() || r[c].is_empty())
                .count();
            n * 100 >= recs.len() * 95 && recs.iter().any(|r| !r[c].is_empty())
        })
        .count();
    if numeric_cols * 3 < w || numeric_cols == 0 {
        return None;
    }
    // The first line is data too, not a header: a header row would have been read already.
    Some(Table {
        format: Format::Headerless,
        headers: generic_headers(w),
        rows: recs,
        notes: Vec::new(),
    })
}

// ---- record blocks -----------------------------------------------------------------------------

/// Expanded output, one block per row: `psql -x`, `sqlite3 -line`, MySQL `\G`.
///
/// ```text
/// -[ RECORD 1 ]--+-------      *************************** 1. row ***************************
/// id   | 1                          id: 1                    id = 1
/// name | alice                    name: alice              name = alice
/// ```
pub fn parse_records(text: &str) -> Option<Table> {
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let (sep, by_blank) = if first.starts_with("-[ RECORD") {
        ('|', false)
    } else if first.starts_with('*') && first.contains(" row ") {
        (':', false)
    } else if first.contains(" = ") {
        ('=', true)
    } else {
        return None;
    };
    let is_marker = |l: &str| {
        (sep == '|' && l.starts_with("-[ RECORD"))
            || (sep == ':' && l.starts_with('*') && l.contains(" row "))
    };
    let mut blocks: Vec<Vec<(String, String)>> = Vec::new();
    let mut cur: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let blank = line.trim().is_empty();
        if is_marker(line) || (by_blank && blank) {
            if !cur.is_empty() {
                blocks.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if blank {
            continue;
        }
        match line.split_once(sep) {
            Some((k, v))
                if !k.trim().is_empty()
                    && k.trim().len() <= 64
                    && !k.trim().contains(char::is_whitespace) =>
            {
                cur.push((k.trim().to_string(), v.trim().to_string()));
            }
            // A value that wraps onto the next line.
            _ => {
                let last = cur.last_mut()?;
                last.1.push('\n');
                last.1.push_str(line.trim());
            }
        }
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }
    if blocks.len() < MIN_ROWS {
        return None;
    }
    let headers: Vec<String> = blocks[0].iter().map(|(k, _)| k.clone()).collect();
    if !valid_header(&headers) {
        return None;
    }
    let same = blocks
        .iter()
        .filter(|b| b.len() == headers.len() && b.iter().zip(&headers).all(|((k, _), h)| k == h))
        .count();
    if same * 20 < blocks.len() * 19 {
        return None;
    }
    let rows = blocks
        .into_iter()
        .filter(|b| b.len() == headers.len())
        .map(|b| b.into_iter().map(|(_, v)| v).collect())
        .collect();
    Some(Table {
        format: Format::Records,
        headers,
        rows,
        notes: Vec::new(),
    })
}

// ---- HTML tables -------------------------------------------------------------------------------

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The text of a cell: its tags removed, its entities decoded, its white space folded.
fn cell_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A page that is one `<table>`, as `df.to_html()` writes it. The first row names the columns; a
/// row header (the pandas index) is a column with an empty name, called `index`.
pub fn parse_html_table(text: &str) -> Option<Table> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if !lower.starts_with("<table")
        || !lower.trim_end().ends_with("</table>")
        || lower.matches("<table").count() != 1
    {
        return None;
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<tr") {
        let row_start = pos + start;
        let end = lower[row_start..].find("</tr>").map(|e| row_start + e)?;
        let mut cells = Vec::new();
        let mut p = row_start;
        loop {
            let rest = &lower[p..end];
            let next = match (rest.find("<th"), rest.find("<td")) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => break,
            };
            let open = p + next;
            let gt = lower[open..end].find('>')? + open + 1;
            let close = lower[gt..end].find("</t").map_or(end, |c| gt + c);
            cells.push(cell_text(&t[gt..close]));
            p = close.max(gt);
        }
        rows.push(cells);
        pos = end + 5;
    }
    if rows.len() < MIN_ROWS + 1 {
        return None;
    }
    let mut headers = rows.remove(0);
    if headers.first().is_some_and(String::is_empty) && !headers.iter().any(|h| h == "index") {
        headers[0] = "index".to_string();
    }
    let w = headers.len();
    let ok = rows.iter().filter(|r| r.len() == w).count();
    if ok * 20 < rows.len() * 19 || !valid_header(&headers) {
        return None;
    }
    rows.retain(|r| r.len() == w);
    Some(Table {
        format: Format::Html,
        headers,
        rows,
        notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_python_list_of_dicts() {
        let rows: Vec<String> = (0..30)
            .map(|i| {
                format!(
                    "{{'id': {i}, 'ts': Timestamp('2026-01-01 00:{:02}:00'), 'name': 'it\\'s {i}', 'amount': np.float64({}.5), 'ok': {}, 'note': {}, 'x': nan}}",
                    i % 60,
                    i,
                    if i % 2 == 0 { "True" } else { "False" },
                    if i % 3 == 0 { "None" } else { "'n'" }
                )
            })
            .collect();
        let text = format!("[{}]", rows.join(", "));
        let t = parse_python_repr(&text).expect("repr");
        assert_eq!(t.format, Format::PyRepr);
        assert_eq!(t.rows.len(), 30);
        assert_eq!(t.headers, ["id", "ts", "name", "amount", "ok", "note", "x"]);
        assert_eq!(t.rows[3][1], "2026-01-01 00:03:00");
        assert_eq!(t.rows[3][2], "it's 3");
        assert_eq!(t.rows[3][3], "3.5");
        assert_eq!(t.rows[3][4], "false");
        assert_eq!(t.rows[3][5], "");
        assert_eq!(t.rows[3][6], "");
    }

    #[test]
    fn keeps_unknown_calls_as_text_and_reads_a_dict_of_lists() {
        let n = 20;
        let a: Vec<String> = (0..n).map(|i| i.to_string()).collect();
        let b: Vec<String> = (0..n)
            .map(|i| format!("datetime.datetime(2026, 1, {}, 0, 0)", i % 28 + 1))
            .collect();
        let text = format!("{{'a': [{}], 'b': [{}]}}", a.join(", "), b.join(", "));
        let t = parse_python_repr(&text).expect("dict of lists");
        assert_eq!(t.rows.len(), 20);
        assert_eq!(t.rows[2][1], "datetime.datetime(2026, 1, 3, 0, 0)");
    }

    #[test]
    fn python_repr_must_be_all_of_the_text() {
        let rows: Vec<String> = (0..30).map(|i| format!("{{'a': {i}, 'b': 'x'}}")).collect();
        let ok = format!("[{}]", rows.join(", "));
        assert!(parse_python_repr(&ok).is_some());
        assert!(parse_python_repr(&format!("{ok} trailing")).is_none());
        assert!(parse_python_repr(&format!("{} oops", &ok[..ok.len() - 1])).is_none());
        assert!(parse_python_repr("[{'a': foo(1}]").is_none());
        // unequal lists are not a table
        assert!(parse_python_repr("{'a': [1, 2, 3], 'b': [1, 2]}").is_none());
    }

    #[test]
    fn reads_pandas_split_json_and_arrays_with_a_header() {
        let data: Vec<Value> = (0..20)
            .map(|i| serde_json::json!([i, format!("n{i}"), i as f64 * 1.5]))
            .collect();
        let split = serde_json::json!({"columns": ["id", "name", "score"], "index": (0..20).collect::<Vec<_>>(), "data": data});
        let t = parse_columnar(&split).expect("split");
        assert_eq!(t.headers, ["id", "name", "score"]);
        assert_eq!(t.rows.len(), 20);
        assert_eq!(t.rows[4], ["4", "n4", "6.0"]);
        let mut arr = vec![serde_json::json!(["id", "name", "score"])];
        arr.extend(data.iter().cloned());
        let t = parse_columnar(&Value::Array(arr)).expect("array of arrays");
        assert_eq!(t.rows.len(), 20);
        // a list of lists of words is not a table
        let words: Vec<Value> = (0..20)
            .map(|i| serde_json::json!(["a", format!("w{i}"), "c"]))
            .collect();
        let mut arr = vec![serde_json::json!(["x", "y", "z"])];
        arr.extend(words);
        assert!(parse_columnar(&Value::Array(arr)).is_none());
        // columns that do not match the rows
        let bad = serde_json::json!({"columns": ["a", "b"], "data": data});
        assert!(parse_columnar(&bad).is_none());
    }

    #[test]
    fn reads_a_printed_matrix() {
        let mut s = String::from("[[");
        for r in 0..30 {
            if r > 0 {
                s.push_str(" [");
            }
            s.push_str(
                &(0..6)
                    .map(|c| format!("{:.4}", (r * 6 + c) as f64 / 7.0))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            s.push_str(if r == 29 { "]]\n" } else { "]\n" });
        }
        let t = parse_number_grid(&s).expect("matrix");
        assert_eq!(t.format, Format::Grid);
        assert_eq!(t.headers, ["c0", "c1", "c2", "c3", "c4", "c5"]);
        assert_eq!(t.rows.len(), 30);
        assert_eq!(t.rows[1][0], "0.8571");
    }

    #[test]
    fn a_grid_is_numbers_only_and_at_least_two_wide() {
        let one: String = (0..50).map(|i| format!("{i}\n")).collect();
        assert!(parse_number_grid(&one).is_none(), "one column is a list");
        let mixed: String = (0..50).map(|i| format!("{i} x{i} 3\n")).collect();
        assert!(parse_number_grid(&mixed).is_none());
        let ips: String = (0..50).map(|i| format!("10.0.0.{i} 8080\n")).collect();
        assert!(
            parse_number_grid(&ips).is_none(),
            "10.0.0.1 is not a number"
        );
    }

    #[test]
    fn reads_delimited_text_with_no_header() {
        let s: String = (0..40)
            .map(|i| format!("{i}|user{}|{}.5|2026-01-0{}|ok\n", i % 5, i * 3, i % 9 + 1))
            .collect();
        let t = parse_headerless(&s).expect("psql -t -A");
        assert_eq!(t.format, Format::Headerless);
        assert_eq!(t.headers.len(), 5);
        assert_eq!(t.rows.len(), 40);
        assert_eq!(t.rows[0][0], "0");
    }

    #[test]
    fn prose_and_word_lists_are_not_headerless_tables() {
        let prose: String = (0..40)
            .map(|i| format!("Well, this is line {i}, and it goes on, truly\n"))
            .collect();
        assert!(parse_headerless(&prose).is_none());
        let words: String = (0..40).map(|i| format!("alpha{i},beta,gamma\n")).collect();
        assert!(
            parse_headerless(&words).is_none(),
            "no column holds numbers"
        );
        let ragged: String = (0..40)
            .map(|i| {
                format!(
                    "{}\n",
                    (0..(i % 5 + 3))
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect();
        assert!(
            parse_headerless(&ragged).is_none(),
            "the field count changes"
        );
    }

    #[test]
    fn reads_psql_expanded_output() {
        let mut s = String::new();
        for i in 0..20 {
            s.push_str(&format!("-[ RECORD {} ]--+-----------------\nid           | {i}\nname         | user | {i}\namount       | {}.5\n", i + 1, i * 3));
        }
        let t = parse_records(&s).expect("psql -x");
        assert_eq!(t.format, Format::Records);
        assert_eq!(t.headers, ["id", "name", "amount"]);
        assert_eq!(t.rows.len(), 20);
        assert_eq!(
            t.rows[4],
            ["4", "user | 4", "12.5"],
            "a value may hold the separator"
        );
    }

    #[test]
    fn reads_sqlite_line_mode_and_mysql_vertical_output() {
        let mut s = String::new();
        for i in 0..15 {
            s.push_str(&format!(
                "     id = {i}\n   name = n{i}\n  score = {}.25\n\n",
                i * 2
            ));
        }
        let t = parse_records(&s).expect("sqlite -line");
        assert_eq!(t.rows.len(), 15);
        assert_eq!(t.rows[3], ["3", "n3", "6.25"]);
        let mut m = String::new();
        for i in 0..15 {
            m.push_str(&format!("*************************** {}. row ***************************\n   id: {i}\n name: n:{i}\nwhen: 2026-01-0{}\n", i + 1, i % 9 + 1));
        }
        let t = parse_records(&m).expect("mysql \\G");
        assert_eq!(t.headers, ["id", "name", "when"]);
        assert_eq!(t.rows[2][1], "n:2");
    }

    #[test]
    fn records_need_the_same_keys_in_every_block() {
        let mut s = String::new();
        for i in 0..20 {
            if i % 2 == 0 {
                s.push_str(&format!("-[ RECORD {} ]--\nid | {i}\nname | a\n", i + 1));
            } else {
                s.push_str(&format!("-[ RECORD {} ]--\nkey | {i}\ncolor | b\n", i + 1));
            }
        }
        assert!(parse_records(&s).is_none());
        assert!(parse_records("just some text\nwith lines\n").is_none());
        // key = value lines in a config file are one block, not rows
        let cfg: String = (0..30).map(|i| format!("key{i} = value{i}\n")).collect();
        assert!(parse_records(&cfg).is_none());
    }

    #[test]
    fn reads_an_html_table_as_pandas_writes_it() {
        let mut s = String::from("<table border=\"1\" class=\"dataframe\">\n  <thead>\n    <tr style=\"text-align: right;\">\n      <th></th>\n      <th>id</th>\n      <th>name</th>\n      <th>note</th>\n    </tr>\n  </thead>\n  <tbody>\n");
        for i in 0..20 {
            s.push_str(&format!("    <tr>\n      <th>{i}</th>\n      <td>{}</td>\n      <td>a &amp; b {i}</td>\n      <td><b>x</b></td>\n    </tr>\n", 100 + i));
        }
        s.push_str("  </tbody>\n</table>\n");
        let t = parse_html_table(&s).expect("html");
        assert_eq!(t.format, Format::Html);
        assert_eq!(t.headers, ["index", "id", "name", "note"]);
        assert_eq!(t.rows.len(), 20);
        assert_eq!(t.rows[3], ["3", "103", "a & b 3", "x"]);
    }

    #[test]
    fn html_that_is_a_page_or_two_tables_is_left_alone() {
        let row = "<tr><td>1</td><td>2</td></tr>".repeat(30);
        assert!(
            parse_html_table(&format!("<html><body><table>{row}</table></body></html>")).is_none()
        );
        assert!(parse_html_table(&format!("<table>{row}</table><table>{row}</table>")).is_none());
        assert!(parse_html_table("not html").is_none());
    }
}
