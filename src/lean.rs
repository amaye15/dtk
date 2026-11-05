//! Lean views: a small, typed summary of a table that keeps what an agent acts on.

use crate::table::{Format, Table};
use std::collections::{HashMap, HashSet};

/// Tables up to this many rows are re-encoded as CSV and nothing is dropped.
pub const LOSSLESS_ROWS: usize = 60;
/// ...but only when that CSV is no bigger than this many estimated tokens.
const LOSSLESS_TOKENS: usize = 1500;
/// Characters kept of a cell in a row that reports a problem (a log's error).
const PROBLEM_CELL: usize = 170;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Int,
    Float,
    Bool,
    Time,
    Str,
    Empty,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Bool => "bool",
            Kind::Time => "time",
            Kind::Str => "str",
            Kind::Empty => "empty",
        }
    }
}

/// The numbers behind a column summary, for programs that want them (`dtk view --json`, tests).
#[derive(Debug, Clone, Default)]
pub struct ColStats {
    pub nonempty: usize,
    pub nulls: usize,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    /// Most common values with their counts, most common first (at most 12).
    pub top: Vec<(String, usize)>,
    /// For text with many distinct values: the shapes the values share (digits and ids replaced
    /// by `#` and `<hex>`), with how many rows have each shape. Most common first (at most 8).
    pub templates: Vec<(String, usize)>,
    /// Cells such as `N/A` or `-` in a column of numbers, dates or flags: how many.
    pub placeholders: usize,
    /// How numbers are written, when not plainly: currency, percent, thousands separators.
    pub written_as: Option<&'static str>,
}

pub struct ColInfo {
    pub kind: Kind,
    /// A column of codes or labels: its values are listed with counts, not as a range.
    pub few_values: bool,
    pub summary: String,
    /// Distinct count when known and small enough to group by.
    pub distinct: Option<usize>,
    pub stats: ColStats,
}

/// Where the full data lives, so the summary can point at it.
pub struct Ctx {
    pub bytes: usize,
    pub csv_path: Option<String>,
    pub raw_path: Option<String>,
}

pub struct Rendered {
    pub text: String,
    /// True when no row and no column was dropped or cut.
    pub lossless: bool,
}

/// Cheap token estimate: four bytes per token.
pub fn est_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

pub fn human_bytes(n: usize) -> String {
    let n = n as f64;
    if n < 1024.0 {
        format!("{}B", n as usize)
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1}KB", n / 1024.0)
    } else {
        format!("{:.1}MB", n / 1024.0 / 1024.0)
    }
}

fn looks_time(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 10 {
        return false;
    }
    let d = |i: usize| b[i].is_ascii_digit();
    d(0) && d(1)
        && d(2)
        && d(3)
        && b[4] == b'-'
        && d(5)
        && d(6)
        && b[7] == b'-'
        && d(8)
        && d(9)
        && (b.len() == 10 || b[10] == b'T' || b[10] == b' ')
}

fn is_bool(s: &str) -> bool {
    matches!(s, "true" | "false" | "True" | "False" | "TRUE" | "FALSE")
}

/// "007" is an identifier, not the number 7.
fn has_leading_zero(s: &str) -> bool {
    let b = s.as_bytes();
    let s = if b.first() == Some(&b'-') { &s[1..] } else { s };
    let b = s.as_bytes();
    b.len() > 1 && b[0] == b'0' && b[1].is_ascii_digit()
}

/// A computed number for a summary: whole numbers whole; four decimals from 0.1 up; below that,
/// four significant digits, so a column of 0.000031 does not read as 0; below 1e-4 the exponent
/// form (`2.96e-8`), which is shorter than the zeros and loses nothing.
pub fn fnum(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let a = x.abs();
    if !(1e-4..1e15).contains(&a) {
        return exp4(x);
    }
    let decimals = if a >= 0.1 {
        4
    } else {
        (3 - a.log10().floor() as i32) as usize
    };
    trim_zeros(format!("{x:.decimals$}"))
}

/// Four significant digits in exponent form, without trailing zeros: `2.96e-8`, `-1.5e20`.
fn exp4(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return "0".into();
    }
    let s = format!("{x:.3e}");
    match s.split_once('e') {
        Some((m, e)) => format!("{}e{e}", trim_zeros(m.to_string())),
        None => s,
    }
}

fn trim_zeros(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// The smallest or largest value of a float column. When it is a short plain number in the data,
/// it is the cell as written: it is a value somebody may quote, and the head and tail rows show
/// the same cells. Otherwise it is the number, rounded as the other statistics are.
fn edge_num(cell: Option<&str>, x: f64) -> String {
    match cell.map(str::trim) {
        // pandas pads to six decimals (`169.717200`); the zeros say nothing.
        Some(c) if c.len() <= 12 && c.parse::<f64>().is_ok() && !c.contains(['e', 'E']) => {
            trim_zeros(c.to_string())
        }
        Some(c) if c.len() <= 12 && c.parse::<f64>().is_ok() => c.to_string(),
        _ => fnum(x),
    }
}

fn top_values(distinct: &HashMap<&str, usize>, limit: usize) -> String {
    const BUDGET: usize = 240;
    let mut v: Vec<(&&str, &usize)> = distinct.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut parts: Vec<String> = Vec::new();
    let mut used = 0;
    for (i, (k, n)) in v.iter().take(limit).enumerate() {
        let part = format!("{} {}", k.chars().take(30).collect::<String>(), n);
        // Long values (titles, paths) stop the list; the three most common always show.
        if i >= 3 && used + part.len() > BUDGET {
            let left = v.len().min(limit) - i;
            parts.push(format!("+{left} more"));
            break;
        }
        used += part.len() + 2;
        parts.push(part);
    }
    parts.join(", ")
}

/// Do all these values look like ids made by a program: hex of one length (a commit, a hash), or
/// UUIDs? Then two examples are noise, and the kind is the information.
fn opaque_kind(distinct: &HashMap<&str, usize>) -> Option<String> {
    let mut len = None;
    for k in distinct.keys().take(50) {
        let hex = k.len() >= 12 && k.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
        if !hex || len.is_some_and(|l| l != k.len()) {
            return None;
        }
        len = Some(k.len());
    }
    let l = len?;
    Some(if l == 36 {
        "uuids".into()
    } else {
        format!("{l}-character hex ids")
    })
}

/// " p50 X p95 Y" (nearest rank), or nothing when there are too few values to mean much.
fn pct(nums: &mut [f64]) -> Option<(f64, f64)> {
    if nums.len() < 20 {
        return None;
    }
    nums.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f64| nums[((nums.len() - 1) as f64 * p).round() as usize];
    Some((at(0.5), at(0.95)))
}

fn pct_text(p: Option<(f64, f64)>) -> String {
    p.map(|(a, b)| format!(" p50 {} p95 {}", fnum(a), fnum(b)))
        .unwrap_or_default()
}

fn is_uuid(c: &[char]) -> bool {
    c.len() == 36
        && c.iter().enumerate().all(|(i, ch)| match i {
            8 | 13 | 18 | 23 => *ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

/// The shape of a text value: digit runs become `#`, long hex ids `<hex>`, uuids `<uuid>`.
/// `request 4821 took 55ms for 9f8e7d6c5b4a` becomes `request # took #ms for <hex>`.
pub fn template(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at_start = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if at_start && c.is_ascii_hexdigit() && i + 36 <= chars.len() && is_uuid(&chars[i..i + 36])
        {
            out.push_str("<uuid>");
            i += 36;
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let tok = &chars[start..i];
        if tok.iter().all(char::is_ascii_digit) {
            out.push('#');
        } else if tok.len() >= 8
            && tok.iter().all(char::is_ascii_hexdigit)
            && tok.iter().any(char::is_ascii_digit)
            && tok.iter().any(char::is_ascii_alphabetic)
        {
            out.push_str("<hex>");
        } else {
            let mut prev_digit = false;
            for &t in tok {
                if t.is_ascii_digit() {
                    if !prev_digit {
                        out.push('#');
                    }
                    prev_digit = true;
                } else {
                    out.push(t);
                    prev_digit = false;
                }
            }
        }
    }
    out
}

/// Template counts over every value, for a column too varied to keep its distinct values.
/// Empty when the column has more than `MAX_SHAPES` shapes.
fn shapes_of_all<'a>(vals: impl Iterator<Item = &'a str>) -> Vec<(String, usize)> {
    const MAX_SHAPES: usize = 2000;
    let mut by: HashMap<String, usize> = HashMap::new();
    for v in vals.filter(|v| !v.is_empty()) {
        let t = template(v);
        if let Some(c) = by.get_mut(&t) {
            *c += 1;
        } else if by.len() < MAX_SHAPES {
            by.insert(t, 1);
        } else {
            return Vec::new();
        }
    }
    let mut v: Vec<(String, usize)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// Row counts per template, most common first.
fn template_counts(distinct: &HashMap<&str, usize>) -> Vec<(String, usize)> {
    let mut by: HashMap<String, usize> = HashMap::new();
    for (v, n) in distinct {
        *by.entry(template(v)).or_default() += n;
    }
    let mut v: Vec<(String, usize)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn top_pairs(distinct: &HashMap<&str, usize>, limit: usize) -> Vec<(String, usize)> {
    let mut v: Vec<(&&str, &usize)> = distinct.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.into_iter()
        .take(limit)
        .map(|(k, n)| (k.to_string(), *n))
        .collect()
}

/// The order of the parts of a date written with slashes, dashes or dots and the year last.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DateOrder {
    MonthFirst,
    DayFirst,
}

impl DateOrder {
    fn label(self) -> &'static str {
        match self {
            DateOrder::MonthFirst => "MM/DD/YYYY",
            DateOrder::DayFirst => "DD/MM/YYYY",
        }
    }
}

/// `3/21/2025`, `21.03.2025`, `21-03-2025` as (first, second, year), or `None`.
fn slash_parts(v: &str) -> Option<(u32, u32, u32)> {
    let sep = v.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let mut it = v.trim().split(sep);
    let (a, b, y) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some()
        || !(1..=2).contains(&a.len())
        || !(1..=2).contains(&b.len())
        || y.len() != 4
        || !(a.bytes().chain(b.bytes()).chain(y.bytes())).all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((a.parse().ok()?, b.parse().ok()?, y.parse().ok()?))
}

/// A column of dates whose order the data settles: some value has a first part above 12 (day
/// first) or a second part above 12 (month first), and none says the opposite. A column where
/// every part is 12 or less could be either, so it is left as text.
fn slash_date_order<'a>(vals: impl Iterator<Item = &'a str>) -> Option<DateOrder> {
    let (mut month_first_ok, mut day_first_ok, mut n) = (true, true, 0usize);
    for v in vals.filter(|v| !v.is_empty() && !is_placeholder(v)) {
        let (a, b, y) = slash_parts(v)?;
        if !(1..=31).contains(&a) || !(1..=31).contains(&b) || !(1000..=2999).contains(&y) {
            return None;
        }
        if a > 12 {
            month_first_ok = false;
        }
        if b > 12 {
            day_first_ok = false;
        }
        n += 1;
    }
    match (n >= 8, month_first_ok, day_first_ok) {
        (true, true, false) => Some(DateOrder::MonthFirst),
        (true, false, true) => Some(DateOrder::DayFirst),
        _ => None,
    }
}

fn slash_to_iso(v: &str, order: DateOrder) -> String {
    match slash_parts(v) {
        Some((a, b, y)) => {
            let (m, d) = if order == DateOrder::MonthFirst {
                (a, b)
            } else {
                (b, a)
            };
            format!("{y:04}-{m:02}-{d:02}")
        }
        None => v.to_string(), // an empty cell or a placeholder
    }
}

/// How a number is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NumFmt {
    Plain,
    Thousands,
    Currency,
    Percent,
}

/// Reads a number as it is written in real data: `12.5`, `1,234`, `$1,234.56`, `45.3%`, `(12.5)`
/// (accounting negative). A value with a leading zero (`007`, a zip code) is not a number.
/// The thousands separator must be exact (`1,234`, not `1,23` or `12,34`), so that a European
/// decimal comma is not read as one.
pub fn parse_number(s: &str) -> Option<(f64, NumFmt)> {
    let t = s.trim();
    if t.is_empty() || has_leading_zero(t) {
        return None;
    }
    if let Ok(f) = t.parse::<f64>() {
        return f.is_finite().then_some((f, NumFmt::Plain));
    }
    let (paren, t) = match t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        Some(r) => (true, r),
        None => (false, t),
    };
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (cur, t) = match t.chars().next() {
        Some(c) if "$€£¥".contains(c) => (true, &t[c.len_utf8()..]),
        _ => (false, t),
    };
    let (neg2, t) = match t.strip_prefix('-') {
        Some(r) if cur => (true, r),
        _ => (false, t),
    };
    let (pct, t) = match t.strip_suffix('%') {
        Some(r) => (true, r),
        None => (false, t),
    };
    if cur && pct {
        return None;
    }
    let (int_part, frac) = match t.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (t, None),
    };
    if int_part.is_empty()
        || frac.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let thousands = int_part.contains(',');
    if thousands {
        let mut groups = int_part.split(',');
        let first = groups.next()?;
        if !(1..=3).contains(&first.len()) || !first.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if !groups.all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
    } else if !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if has_leading_zero(&int_part.replace(',', "")) {
        return None;
    }
    let digits = match frac {
        Some(f) => format!("{}.{f}", int_part.replace(',', "")),
        None => int_part.replace(',', ""),
    };
    let mut f: f64 = digits.parse().ok()?;
    if neg ^ neg2 ^ paren {
        f = -f;
    }
    let fmt = if cur {
        NumFmt::Currency
    } else if pct {
        NumFmt::Percent
    } else if thousands {
        NumFmt::Thousands
    } else {
        // Only a sign or parentheses made it differ from a plain number.
        NumFmt::Plain
    };
    Some((f, fmt))
}

/// Spellings that mean "no value" in a column that is otherwise numbers, times or flags.
const PLACEHOLDERS: [&str; 13] = [
    "n/a", "na", "nan", "null", "none", "nil", "-", "--", "?", "#n/a", "<na>", "(null)", "missing",
];

pub fn is_placeholder(v: &str) -> bool {
    let t = v.trim();
    !t.is_empty() && PLACEHOLDERS.iter().any(|p| t.eq_ignore_ascii_case(p))
}

/// A column of numbers with `N/A` in some cells is a column of numbers. Placeholders are counted
/// and set aside when the rest of the column is numbers, times or flags, and more than half of
/// the cells hold real values. Otherwise they are ordinary values.
fn analyze_col<'a>(vals: impl Iterator<Item = &'a str> + Clone) -> ColInfo {
    // `03/21/2025`: dates in the order the data itself settles. Read as ISO dates, they get a
    // range like any time column.
    if let Some(order) = slash_date_order(vals.clone()) {
        let iso: Vec<String> = vals.clone().map(|v| slash_to_iso(v, order)).collect();
        let mut c = analyze_col(iso.iter().map(String::as_str));
        put_note(&mut c.summary, &format!("dates written {}", order.label()));
        return c;
    }
    if vals.clone().any(is_placeholder) {
        let c = analyze_col_impl(vals.clone(), true);
        let typed = matches!(c.kind, Kind::Int | Kind::Float | Kind::Bool | Kind::Time);
        if typed && c.stats.placeholders < c.stats.nonempty {
            return c;
        }
    }
    analyze_col_impl(vals, false)
}

fn analyze_col_impl<'a>(vals: impl Iterator<Item = &'a str> + Clone, skip: bool) -> ColInfo {
    let second_pass = vals.clone();
    let mut nonempty = 0usize;
    let mut nulls = 0usize;
    let (mut all_int, mut all_float, mut all_bool, mut all_time) = (true, true, true, true);
    let (mut imin, mut imax) = (i64::MAX, i64::MIN);
    let (mut fmin, mut fmax, mut fsum) = (f64::INFINITY, f64::NEG_INFINITY, 0f64);
    let (mut fmin_cell, mut fmax_cell): (Option<&str>, Option<&str>) = (None, None);
    let (mut tmin, mut tmax): (Option<&str>, Option<&str>) = (None, None);
    let mut distinct: HashMap<&str, usize> = HashMap::new();
    let mut overflow = false;
    let mut nums: Vec<f64> = Vec::new();
    let mut placeholders: HashMap<&str, usize> = HashMap::new();
    let mut fmt_seen = [0usize; 4];
    for v in vals {
        if v.is_empty() {
            nulls += 1;
            continue;
        }
        if skip && is_placeholder(v) {
            *placeholders.entry(v).or_default() += 1;
            continue;
        }
        nonempty += 1;
        if all_float {
            match parse_number(v) {
                Some((f, fmt)) => {
                    fmt_seen[fmt as usize] += 1;
                    if f < fmin {
                        fmin_cell = (fmt == NumFmt::Plain).then_some(v);
                    }
                    if f > fmax {
                        fmax_cell = (fmt == NumFmt::Plain).then_some(v);
                    }
                    fmin = fmin.min(f);
                    fmax = fmax.max(f);
                    fsum += f;
                    nums.push(f);
                    if all_int {
                        let integral = f.fract() == 0.0 && !v.contains('.');
                        match (fmt, v.parse::<i64>()) {
                            (NumFmt::Plain, Ok(i)) => {
                                imin = imin.min(i);
                                imax = imax.max(i);
                            }
                            (NumFmt::Plain, Err(_)) => all_int = false,
                            _ if integral => {
                                imin = imin.min(f as i64);
                                imax = imax.max(f as i64);
                            }
                            _ => all_int = false,
                        }
                    }
                }
                None => {
                    all_float = false;
                    all_int = false;
                    nums = Vec::new();
                }
            }
        }
        if all_bool && !is_bool(v) {
            all_bool = false;
        }
        if all_time {
            if looks_time(v) {
                if tmin.map_or(true, |m| v < m) {
                    tmin = Some(v);
                }
                if tmax.map_or(true, |m| v > m) {
                    tmax = Some(v);
                }
            } else {
                all_time = false;
            }
        }
        if !overflow {
            if let Some(c) = distinct.get_mut(v) {
                *c += 1;
            } else if distinct.len() >= 5000 {
                overflow = true;
            } else {
                distinct.insert(v, 1);
            }
        }
    }
    // A column mixes at most one way of writing numbers, and a percent column is all percents.
    let non_plain = [NumFmt::Thousands, NumFmt::Currency, NumFmt::Percent]
        .iter()
        .filter(|f| fmt_seen[**f as usize] > 0)
        .count();
    if non_plain > 1
        || (fmt_seen[NumFmt::Percent as usize] > 0 && fmt_seen[NumFmt::Plain as usize] > 0)
    {
        all_float = false;
        all_int = false;
    }
    let written_as = if all_float && nonempty > 0 {
        if fmt_seen[NumFmt::Currency as usize] > 0 {
            Some("currency")
        } else if fmt_seen[NumFmt::Percent as usize] > 0 {
            Some("percent")
        } else if fmt_seen[NumFmt::Thousands as usize] > 0 {
            Some("thousands separators")
        } else {
            None
        }
    } else {
        None
    };
    let kind = if nonempty == 0 {
        Kind::Empty
    } else if all_int {
        Kind::Int
    } else if all_float {
        Kind::Float
    } else if all_bool {
        Kind::Bool
    } else if all_time {
        Kind::Time
    } else {
        Kind::Str
    };
    let mean = if nonempty > 0 {
        fsum / nonempty as f64
    } else {
        0.0
    };
    // Codes such as HTTP status or a rating: counts say more than a min, max and mean.
    let few_values = !overflow && distinct.len() <= 12 && nonempty >= 2 * distinct.len();
    // Text with many distinct values (messages with ids and numbers in them): group by shape.
    let templates = if kind == Kind::Str && !overflow && distinct.len() > 12 {
        template_counts(&distinct)
    } else if kind == Kind::Str && overflow {
        // More than 5,000 different values, which is when the shapes matter most: count the
        // shapes over every row. A column with thousands of shapes has none worth showing.
        shapes_of_all(second_pass)
    } else {
        Vec::new()
    };
    let pcts = if matches!(kind, Kind::Int | Kind::Float) {
        pct(&mut nums)
    } else {
        None
    };
    let mut summary = match kind {
        Kind::Empty => "all empty".to_string(),
        Kind::Int | Kind::Float if few_values => {
            format!("{} distinct: {}", distinct.len(), top_values(&distinct, 12))
        }
        Kind::Int => format!("{}..{} mean {}{}", imin, imax, fnum(mean), pct_text(pcts)),
        Kind::Float => format!(
            "{}..{} mean {}{}",
            edge_num(fmin_cell, fmin),
            edge_num(fmax_cell, fmax),
            fnum(mean),
            pct_text(pcts)
        ),
        Kind::Time => format!("{}..{}", tmin.unwrap_or(""), tmax.unwrap_or("")),
        Kind::Bool => top_values(&distinct, 2),
        Kind::Str => {
            if overflow {
                if templates.len() >= 2 && templates.len() <= 200 {
                    let shown: Vec<String> = templates
                        .iter()
                        .take(6)
                        .map(|(t, n)| format!("{} ({})", t.chars().take(60).collect::<String>(), n))
                        .collect();
                    let more = if templates.len() > 6 {
                        format!(", +{} more", templates.len() - 6)
                    } else {
                        String::new()
                    };
                    format!(
                        ">5000 distinct, {} templates: {}{}",
                        templates.len(),
                        shown.join("; "),
                        more
                    )
                } else {
                    ">5000 distinct".to_string()
                }
            } else if let (true, Some(kind)) = (distinct.len() >= 3, opaque_kind(&distinct)) {
                // a few hashes: the kind says it; a count says how often the commonest recurs
                let most = distinct.values().max().copied().unwrap_or(0);
                format!(
                    "{} distinct, {kind}; the commonest appears {most}x",
                    distinct.len()
                )
            } else if distinct.len() <= 12 {
                format!("{} distinct: {}", distinct.len(), top_values(&distinct, 12))
            } else if templates.len() >= 2 && templates.len() * 3 <= distinct.len() {
                let shown: Vec<String> = templates
                    .iter()
                    .take(6)
                    .map(|(t, n)| format!("{} ({})", t.chars().take(60).collect::<String>(), n))
                    .collect();
                let more = if templates.len() > 6 {
                    format!(", +{} more", templates.len() - 6)
                } else {
                    String::new()
                };
                format!(
                    "{} distinct, {} templates: {}{}",
                    distinct.len(),
                    templates.len(),
                    shown.join("; "),
                    more
                )
            } else {
                let unique = if distinct.len() == nonempty {
                    ", all unique"
                } else {
                    ""
                };
                match opaque_kind(&distinct) {
                    Some(kind) => format!("{} distinct{unique}, {kind}", distinct.len()),
                    None => format!(
                        "{} distinct{}, e.g. {}",
                        distinct.len(),
                        unique,
                        top_values(&distinct, 2)
                    ),
                }
            }
        }
    };
    if let (Some(w), true) = (written_as, matches!(kind, Kind::Int | Kind::Float)) {
        summary.push_str(&format!(" | written with {w}"));
    }
    if nulls > 0 {
        summary.push_str(&format!(" | nulls {}", nulls));
    }
    let placeholder_total: usize = placeholders.values().sum();
    if placeholder_total > 0 {
        let mut forms: Vec<(&&str, &usize)> = placeholders.iter().collect();
        forms.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let shown: Vec<String> = forms
            .iter()
            .take(4)
            .map(|(f, n)| format!("{f} {n}"))
            .collect();
        summary.push_str(&format!(
            " | {placeholder_total} placeholders excluded ({})",
            shown.join(", ")
        ));
    }
    let numeric = matches!(kind, Kind::Int | Kind::Float);
    let stats = ColStats {
        nonempty,
        nulls,
        min: numeric.then(|| if kind == Kind::Int { imin as f64 } else { fmin }),
        max: numeric.then(|| if kind == Kind::Int { imax as f64 } else { fmax }),
        mean: numeric.then_some(mean),
        p50: pcts.map(|p| p.0),
        p95: pcts.map(|p| p.1),
        top: top_pairs(&distinct, 12),
        templates: templates.into_iter().take(8).collect(),
        placeholders: placeholder_total,
        written_as: if numeric { written_as } else { None },
    };
    ColInfo {
        kind,
        few_values,
        summary,
        distinct: if overflow { None } else { Some(distinct.len()) },
        stats,
    }
}

pub fn analyze(t: &Table) -> Vec<ColInfo> {
    let mut info: Vec<ColInfo> = (0..t.headers.len())
        .map(|c| analyze_col(t.rows.iter().map(|r| r[c].as_str())))
        .collect();
    add_extremes(t, &mut info);
    add_cadence(t, &mut info);
    // A wide table has dozens of statistics lines. Keep each short: the quantiles can be had
    // for any column with `dtk stats`.
    if t.headers.len() > WIDE_COLS {
        for i in info.iter_mut() {
            if let (Some(p50), Some(p95)) = (i.stats.p50, i.stats.p95) {
                i.summary = i.summary.replacen(&pct_text(Some((p50, p95))), "", 1);
            }
        }
    }
    info
}

/// Above this many columns a table gets statistics for only a dozen of them.
const VERY_WIDE_COLS: usize = 40;

/// A table with more columns than this is "wide": its statistics lines drop p50 and p95.
const WIDE_COLS: usize = 24;

/// Most columns that get a "which row?" note. More would cost more than they are worth.
const EXTREME_COLS: usize = 5;
/// Most columns that get an outlier note (only columns that have outliers cost anything).
const OUTLIER_COLS: usize = 5;
/// An id longer than this is a blob, not a name for a row.
const ID_MAX_CHARS: usize = 40;

/// The column that names a row: the first of the first three whose values are all different, not
/// empty and short (an order id, a timestamp). `None` means rows are named by their position.
pub fn id_column(t: &Table, info: &[ColInfo]) -> Option<usize> {
    let n = t.rows.len();
    (0..t.headers.len().min(3)).find(|&c| {
        info[c].distinct == Some(n)
            && info[c].stats.nulls == 0
            && t.rows.iter().all(|r| r[c].chars().count() <= ID_MAX_CHARS)
    })
}

/// Does a column name say "identifier"? `id`, `order_id`, `userId`, `uuid`, `index`.
pub(crate) fn id_like_name(name: &str) -> bool {
    let n = name.to_lowercase();
    matches!(
        n.as_str(),
        "id" | "uuid" | "guid" | "index" | "idx" | "key" | "pk" | "rowid" | "#" | "no" | "num"
    ) || n.ends_with("_id")
        || n.ends_with("-id")
        || n.ends_with("id") && n.len() > 2 && !n.ends_with("paid") && !n.ends_with("valid")
}

/// The numbers of a column, in row order, with the row each came from.
fn numbers_of(t: &Table, c: usize) -> Vec<(usize, f64)> {
    t.rows
        .iter()
        .enumerate()
        .filter_map(|(r, row)| parse_number(&row[c]).map(|(f, _)| (r, f)))
        .collect()
}

/// A column that numbers or names rows, not one that measures them: the largest id is not
/// news. Its name says so, or it rises (or falls) at every row.
fn identifier_like(t: &Table, info: &[ColInfo], c: usize, vals: &[(usize, f64)]) -> bool {
    if id_like_name(&t.headers[c]) && info[c].kind == Kind::Int {
        return true;
    }
    let n = vals.len();
    n >= 20
        && info[c].distinct == Some(n)
        && (vals.windows(2).all(|w| w[0].1 < w[1].1) || vals.windows(2).all(|w| w[0].1 > w[1].1))
}

/// Adds a note to a column's summary after its statistics and before the ` | ...` flags
/// (nulls, placeholders, how the numbers are written).
fn put_note(summary: &mut String, note: &str) {
    match summary.find(" | ") {
        Some(i) => summary.insert_str(i, &format!(" | {note}")),
        None => summary.push_str(&format!(" | {note}")),
    }
}

/// "Which order was the slowest?" is the first question about a table of numbers, and the min
/// and the max alone do not answer it. Add the row that holds each, named by its id. Add the
/// outliers too: a mean and a p95 hide the two values that are a hundred times the rest.
fn add_extremes(t: &Table, info: &mut [ColInfo]) {
    let id = id_column(t, info);
    let name = |r: usize| match id {
        Some(c) => format!("{}={}", t.headers[c], trunc(&t.rows[r][c], ID_MAX_CHARS)),
        None => format!("row {r}"),
    };
    let (mut extremes, mut outliers) = (0, 0);
    for c in 0..t.headers.len() {
        if extremes == EXTREME_COLS && outliers == OUTLIER_COLS {
            break;
        }
        let numeric = matches!(info[c].kind, Kind::Int | Kind::Float);
        // Codes (HTTP status, ratings) are listed with counts, and an id names rows itself.
        if !numeric || info[c].few_values || Some(c) == id {
            continue;
        }
        let vals = numbers_of(t, c);
        if vals.is_empty() || identifier_like(t, info, c, &vals) {
            continue;
        }
        let (lo, hi) = vals
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), (_, v)| {
                (l.min(*v), h.max(*v))
            });
        if hi <= lo {
            continue;
        }
        let label = |target: f64| {
            let mut hits = vals.iter().filter(|(_, v)| *v == target);
            hits.next().map(|(r, _)| {
                let more = hits.count();
                if more > 0 {
                    format!("{} (+{more} tied)", name(*r))
                } else {
                    name(*r)
                }
            })
        };
        let mut notes: Vec<String> = Vec::new();
        if extremes < EXTREME_COLS {
            if let (Some(max_at), Some(min_at)) = (label(hi), label(lo)) {
                notes.push(format!("max at {max_at}, min at {min_at}"));
                extremes += 1;
            }
        }
        if outliers < OUTLIER_COLS {
            if let Some(note) = outlier_note(&vals) {
                notes.push(note);
                outliers += 1;
            }
        }
        if !notes.is_empty() {
            put_note(&mut info[c].summary, &notes.join(" | "));
        }
    }
}

/// Days from 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since 1970-01-01 for `2026-01-02`, `2026-01-02 03:04`, `2026-01-02T03:04:05.5Z` or
/// with a `+02:00` offset.
fn iso_seconds(s: &str) -> Option<f64> {
    let s = s.trim();
    if !s.is_ascii() || s.len() < 10 {
        return None;
    }
    let b = s.as_bytes();
    let num = |from: usize, n: usize| -> Option<i64> {
        let t = s.get(from..from + n)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())
            .flatten()
    };
    if b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, mo, d) as f64 * 86_400.0;
    if s.len() == 10 {
        return Some(secs);
    }
    if !matches!(b[10], b'T' | b' ') || b.get(13) != Some(&b':') {
        return None;
    }
    secs += (num(11, 2)? * 3600 + num(14, 2)? * 60) as f64;
    let mut i = 16;
    if b.get(i) == Some(&b':') {
        secs += num(i + 1, 2)? as f64;
        i += 3;
        if b.get(i) == Some(&b'.') {
            let digits = s[i + 1..].bytes().take_while(u8::is_ascii_digit).count();
            let frac: f64 = format!("0.{}", &s[i + 1..i + 1 + digits]).parse().ok()?;
            secs += frac;
            i += 1 + digits;
        }
    }
    match b.get(i) {
        None | Some(b'Z') => Some(secs),
        Some(&sign @ (b'+' | b'-')) => {
            let h = num(i + 1, 2)?;
            let m = if b.get(i + 3) == Some(&b':') {
                num(i + 4, 2)?
            } else {
                num(i + 3, 2).unwrap_or(0)
            };
            let off = (h * 3600 + m * 60) as f64;
            Some(if sign == b'+' { secs - off } else { secs + off })
        }
        _ => None,
    }
}

/// A length of time for a person, in at most two units: `37min`, `1h30min`, `2d3h`, `0.5s`.
fn duration(secs: f64) -> String {
    let secs = secs.abs();
    if secs < 1.0 {
        return format!(
            "{}s",
            format!("{secs:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
        );
    }
    let n = secs.round() as u64;
    let parts: Vec<String> = [
        (n / 86_400, "d"),
        (n % 86_400 / 3600, "h"),
        (n % 3600 / 60, "min"),
        (n % 60, "s"),
    ]
    .into_iter()
    .filter(|(v, _)| *v > 0)
    .take(2)
    .map(|(v, u)| format!("{v}{u}"))
    .collect();
    parts.concat()
}

/// For a time column with enough rows: are the rows in time order, are they evenly spaced, and
/// where is the longest gap. A forecast or a monitor depends on all three.
fn add_cadence(t: &Table, info: &mut [ColInfo]) {
    for c in 0..t.headers.len() {
        if info[c].kind != Kind::Time {
            continue;
        }
        let times: Vec<(usize, f64)> = t
            .rows
            .iter()
            .enumerate()
            .filter_map(|(r, row)| iso_seconds(&row[c]).map(|s| (r, s)))
            .collect();
        if times.len() < 20 || times.len() * 10 < t.rows.len() * 9 {
            continue;
        }
        let deltas: Vec<f64> = times.windows(2).map(|w| w[1].1 - w[0].1).collect();
        let (up, down) = (
            deltas.iter().all(|d| *d >= 0.0),
            deltas.iter().all(|d| *d <= 0.0),
        );
        let order = match (up, down) {
            (true, false) => "sorted oldest first",
            (false, true) => "sorted newest first",
            (true, true) => continue, // all the same time
            _ => {
                put_note(&mut info[c].summary, "not in time order");
                continue;
            }
        };
        let mut steps: Vec<f64> = deltas
            .iter()
            .map(|d| d.abs())
            .filter(|d| *d > 0.0)
            .collect();
        if steps.is_empty() {
            continue;
        }
        steps.sort_by(|a, b| a.total_cmp(b));
        let median = steps[steps.len() / 2];
        let even = steps
            .iter()
            .filter(|d| (**d - median).abs() <= 1e-6 * median.max(1.0))
            .count();
        let mut note = if even * 10 >= steps.len() * 9 {
            format!("{order}, every {}", duration(median))
        } else {
            format!("{order}, typical step {}", duration(median))
        };
        let (gap_at, gap) = deltas
            .iter()
            .enumerate()
            .map(|(i, d)| (i, d.abs()))
            .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
        if gap > 3.0 * median && gap >= 2.0 {
            note.push_str(&format!(
                ", longest gap {} after row {}",
                duration(gap),
                times[gap_at].0
            ));
        }
        put_note(&mut info[c].summary, &note);
    }
}

/// Values far outside the rest: beyond three interquartile ranges from the quartiles. A few
/// of them (up to 5% of the rows) are outliers; more than that is a heavy tail, not outliers.
fn outlier_note(vals: &[(usize, f64)]) -> Option<String> {
    let n = vals.len();
    if n < 20 {
        return None;
    }
    let mut sorted: Vec<f64> = vals.iter().map(|(_, v)| *v).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let (q1, q3) = (sorted[(n - 1) / 4], sorted[(3 * (n - 1)) / 4]);
    let iqr = q3 - q1;
    if iqr <= 0.0 {
        return None;
    }
    let (hi_fence, lo_fence) = (q3 + 3.0 * iqr, q1 - 3.0 * iqr);
    let high: Vec<f64> = sorted.iter().copied().filter(|v| *v > hi_fence).collect();
    let low: Vec<f64> = sorted.iter().copied().filter(|v| *v < lo_fence).collect();
    let total = high.len() + low.len();
    if total == 0 || total > (n / 20).max(3) {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(top) = high.last() {
        parts.push(format!(
            "{} above {} (most extreme {})",
            high.len(),
            fnum(hi_fence),
            fnum(*top)
        ));
    }
    if let Some(bottom) = low.first() {
        parts.push(format!(
            "{} below {} (most extreme {})",
            low.len(),
            fnum(lo_fence),
            fnum(*bottom)
        ));
    }
    Some(format!("outliers beyond 3xIQR: {}", parts.join(", ")))
}

pub fn trunc(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn write_csv(header: Option<&[String]>, rows: &[&Vec<String>], max_cell: Option<usize>) -> String {
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    let cut = |s: &String| match max_cell {
        Some(m) => trunc(s, m),
        None => s.clone(),
    };
    if let Some(h) = header {
        let _ = w.write_record(h);
    }
    for r in rows {
        let _ = w.write_record(r.iter().map(cut));
    }
    let bytes = w.into_inner().unwrap_or_default();
    String::from_utf8_lossy(&bytes).trim_end().to_string()
}

/// The whole table as CSV with one header row. Used for the file that holds the full data.
pub fn to_csv(t: &Table) -> String {
    let rows: Vec<&Vec<String>> = t.rows.iter().collect();
    let mut s = write_csv(Some(&t.headers), &rows, None);
    s.push('\n');
    s
}

pub fn shell_quote(p: &str) -> String {
    // Windows accepts forward slashes everywhere, and a backslash path would need quoting.
    let p = if cfg!(windows) {
        p.replace('\\', "/")
    } else {
        p.to_string()
    };
    if p.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~:".contains(c))
    {
        p
    } else {
        format!("'{}'", p.replace('\'', "'\\''"))
    }
}

const LEVEL_NAMES: [&str; 6] = ["level", "severity", "lvl", "loglevel", "log_level", "flag"];
/// Lines shown above each error in a text log.
const CONTEXT_LINES: usize = 4;

const BAD_LEVELS: [&str; 8] = [
    "warn", "warning", "error", "err", "fatal", "critical", "crit", "panic",
];

struct Profile {
    head: usize,
    tail: usize,
    important: usize,
    max_cell: usize,
    stat_cols: usize,
}

const PROFILES: [Profile; 3] = [
    Profile {
        head: 5,
        tail: 2,
        important: 20,
        max_cell: 60,
        stat_cols: 30,
    },
    Profile {
        head: 3,
        tail: 2,
        important: 10,
        max_cell: 40,
        stat_cols: 24,
    },
    Profile {
        head: 2,
        tail: 1,
        important: 5,
        max_cell: 30,
        stat_cols: 12,
    },
];

/// Renders the lean view. Summary mode tries smaller profiles until it fits `budget` tokens.
pub fn render(t: &Table, ctx: &Ctx, budget: usize) -> Rendered {
    render_for(t, ctx, budget, &[])
}

/// The most rows that a summary adds because they carry a word of the request.
const MAX_REQUEST_ROWS: usize = 8;

/// The rows that hold one of `keys` (the words of a request, see `request::keys`) as a whole cell,
/// each with what matched, as `(column = value, row)`. A summary shows few rows, and the row that a
/// model asked for by its key is usually not one of them.
///
/// A match must mean "this row". So only a text column counts, or a column of whole numbers that
/// are all different (an id): the word `3` is not a request for the rows with a quantity of 3.
/// A key that matches more than a fifth of the rows (at least two) names nothing and is skipped.
/// Cells are compared as written, ignoring the case of ASCII letters, with the spaces cut off.
fn request_rows(t: &Table, info: &[ColInfo], keys: &[String]) -> Vec<(String, usize)> {
    if keys.is_empty() || t.rows.is_empty() {
        return Vec::new();
    }
    let n = t.rows.len();
    let eligible: Vec<usize> = (0..t.headers.len())
        .filter(|&c| match info[c].kind {
            Kind::Str => true,
            Kind::Int => {
                let all: std::collections::HashSet<&str> =
                    t.rows.iter().map(|r| r[c].trim()).collect();
                all.len() == n
            }
            _ => false,
        })
        .collect();
    // hits[k] = (column, row) pairs in which a whole cell is key k.
    let mut hits: Vec<Vec<(usize, usize)>> = vec![Vec::new(); keys.len()];
    for &c in &eligible {
        for (r, row) in t.rows.iter().enumerate() {
            let cell = row[c].trim();
            if cell.is_empty() || cell.len() > 80 {
                continue;
            }
            for (k, key) in keys.iter().enumerate() {
                if cell.len() == key.len() && cell.eq_ignore_ascii_case(key) {
                    hits[k].push((c, r));
                }
            }
        }
    }
    let limit = (n / 5).max(2);
    let mut out: Vec<(String, usize)> = Vec::new();
    for h in hits.iter().filter(|h| !h.is_empty()) {
        let mut rows: Vec<usize> = h.iter().map(|&(_, r)| r).collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.len() > limit {
            continue;
        }
        let (c, r0) = h[0];
        let label = format!("{} = {}", t.headers[c], trunc(t.rows[r0][c].trim(), 40));
        for r in rows {
            if !out.iter().any(|(_, x)| *x == r) {
                out.push((label.clone(), r));
                if out.len() >= MAX_REQUEST_ROWS {
                    return out;
                }
            }
        }
    }
    out
}

/// `render`, with the words of the request that produced the output. A row that holds one of them
/// as a whole cell is added to a summary (never to a table that is shown whole). With no word, or
/// no row that matches, the result is exactly that of `render`.
pub fn render_for(t: &Table, ctx: &Ctx, budget: usize, keys: &[String]) -> Rendered {
    let n = t.rows.len();
    let cols = t.headers.len();
    let mut fallback: Option<String> = None;
    if n <= LOSSLESS_ROWS {
        let rows: Vec<&Vec<String>> = t.rows.iter().collect();
        let mut text = format!(
            "[dtk] {} as csv: {} rows x {} cols, nothing dropped\n{}",
            t.format.name(),
            n,
            cols,
            write_csv(Some(&t.headers), &rows, None)
        );
        for note in &t.notes {
            text.push_str(&format!("\nnote: {}", note));
        }
        // Showing every row only pays when the CSV is small and clearly smaller than the source.
        // A wide table with nested JSON cells is neither, and gets the summary instead.
        let (own, source) = (est_tokens(&text), ctx.bytes.div_ceil(4));
        if own <= LOSSLESS_TOKENS && (source == 0 || own * 10 <= source * 7) {
            return Rendered {
                text,
                lossless: true,
            };
        }
        fallback = Some(text);
    }
    let info = analyze(t);
    let wanted = request_rows(t, &info, keys);
    let mut last = String::new();
    for p in PROFILES.iter() {
        last = summary(t, &info, ctx, p, &wanted);
        if est_tokens(&last) <= budget {
            break;
        }
    }
    // A small table that was too wide or too heavy for the full CSV can still be smaller that way
    // than behind the summary's fixed lines.
    if let Some(full) = fallback {
        if est_tokens(&full) < est_tokens(&last) {
            return Rendered {
                text: full,
                lossless: true,
            };
        }
    }
    Rendered {
        text: last,
        lossless: false,
    }
}

/// What a column is worth in a summary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColClass {
    /// It varies: it gets a statistics line.
    Informative,
    /// The same value in (nearly) every row: one line for all of them.
    Constant,
    /// A different token in every row (an id, a URL, a hash): its name is enough.
    Unique,
}

/// Does this column say anything beyond its name? `a=1` in every row, or `https://x/1`,
/// `https://x/2`, ... in different rows, takes a line of statistics and tells the model nothing.
fn classify(t: &Table, info: &[ColInfo], c: usize) -> ColClass {
    let n = t.rows.len();
    let i = &info[c];
    if i.kind == Kind::Empty {
        return ColClass::Constant;
    }
    if i.distinct == Some(1) && i.stats.nonempty >= 2 && i.stats.nulls == 0 {
        return ColClass::Constant;
    }
    // Unique text with no spaces in it and no pattern worth showing.
    if i.kind == Kind::Str
        && i.distinct == Some(i.stats.nonempty)
        && i.stats.nonempty >= 10
        && i.stats.nonempty * 10 >= n * 9
        && t.rows
            .iter()
            .take(200)
            .all(|r| !r[c].contains(char::is_whitespace))
    {
        return ColClass::Unique;
    }
    ColClass::Informative
}

/// Columns that split the rows into exactly the same groups as an earlier column does: the id, the
/// login, the node id and the URL of one actor are one fact, written four ways. Returns, for each
/// such column, the earlier column it is a copy of. Only columns with repeated values are
/// compared: a column of unique values is told apart by `classify`.
fn aliases(t: &Table, info: &[ColInfo], candidates: &[usize]) -> Vec<(usize, usize)> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let n = t.rows.len();
    // canonical group labels: numbered in order of first appearance
    let labels = |c: usize| -> Vec<u32> {
        let mut seen: HashMap<&str, u32> = HashMap::new();
        t.rows
            .iter()
            .map(|r| {
                let next = seen.len() as u32;
                *seen.entry(r[c].as_str()).or_insert(next)
            })
            .collect()
    };
    let mut by_print: HashMap<u64, Vec<(usize, Vec<u32>)>> = HashMap::new();
    let mut out = Vec::new();
    for &c in candidates {
        let d = info[c].distinct.unwrap_or(0);
        if !(2..n).contains(&d) || info[c].stats.nulls > 0 {
            continue;
        }
        let l = labels(c);
        let mut h = DefaultHasher::new();
        l.hash(&mut h);
        let bucket = by_print.entry(h.finish()).or_default();
        if let Some((first, _)) = bucket.iter().find(|(_, other)| *other == l) {
            out.push((c, *first));
        } else {
            bucket.push((c, l));
        }
    }
    out
}

/// Names written once, with a run such as `feat_0`, `feat_1`, ... `feat_59` as `feat_0..feat_59`.
fn compact_names(names: &[&str]) -> String {
    fn split(name: &str) -> Option<(&str, u64)> {
        let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 || digits > 9 {
            return None;
        }
        let (prefix, num) = name.split_at(name.len() - digits);
        Some((prefix, num.parse().ok()?))
    }
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < names.len() {
        let mut j = i;
        if let Some((prefix, mut last)) = split(names[i]) {
            while j + 1 < names.len() {
                match split(names[j + 1]) {
                    Some((p2, n2)) if p2 == prefix && n2 == last + 1 => {
                        last = n2;
                        j += 1;
                    }
                    _ => break,
                }
            }
        }
        if j - i >= 2 {
            out.push(format!("{}..{}", names[i], names[j]));
        } else {
            out.push(names[i].to_string());
            j = i;
        }
        i = j + 1;
    }
    out.join(", ")
}

/// `name:type` for every column. A run of columns that differ only by a counter and share a type
/// (`feat_0`, `feat_1`, ... `feat_119`) is written once: `feat_0..feat_119:float`.
fn compact_cols(headers: &[String], info: &[ColInfo]) -> String {
    // "feat_12" -> ("feat_", 12)
    fn split(name: &str) -> Option<(&str, u64)> {
        let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 || digits > 9 {
            return None;
        }
        let (prefix, num) = name.split_at(name.len() - digits);
        Some((prefix, num.parse().ok()?))
    }
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < headers.len() {
        let kind = info[i].kind.name();
        let mut j = i;
        if let Some((prefix, mut last)) = split(&headers[i]) {
            while j + 1 < headers.len() {
                match split(&headers[j + 1]) {
                    Some((p2, n2))
                        if p2 == prefix && n2 == last + 1 && info[j + 1].kind.name() == kind =>
                    {
                        last = n2;
                        j += 1;
                    }
                    _ => break,
                }
            }
        }
        if j - i >= 2 {
            out.push(format!("{}..{}:{kind}", headers[i], headers[j]));
        } else {
            out.push(format!("{}:{kind}", headers[i]));
            j = i;
        }
        i = j + 1;
    }
    out.join(" ")
}

/// The data file as a command names it.
fn data_ref(ctx: &Ctx) -> String {
    ctx.csv_path
        .as_deref()
        .map_or_else(|| "<data file>".to_string(), file_ref)
}

/// The rows that hold the largest and smallest value of the first measure and the largest of the
/// second, with what each is: `("largest amount", 17)`. One row per label, none twice.
fn notable_rows(t: &Table, info: &[ColInfo]) -> Vec<(String, usize)> {
    let id = id_column(t, info);
    let mut measures: Vec<(usize, Vec<(usize, f64)>)> = Vec::new();
    for c in 0..t.headers.len() {
        if measures.len() == 2 {
            break;
        }
        if !matches!(info[c].kind, Kind::Int | Kind::Float) || info[c].few_values || Some(c) == id {
            continue;
        }
        let vals = numbers_of(t, c);
        if !vals.is_empty() && !identifier_like(t, info, c, &vals) {
            measures.push((c, vals));
        }
    }
    let mut out: Vec<(String, usize)> = Vec::new();
    for (i, (c, vals)) in measures.iter().enumerate() {
        let first = |better: &dyn Fn(f64, f64) -> bool| {
            vals.iter()
                .copied()
                .reduce(|a, b| if better(b.1, a.1) { b } else { a })
        };
        let (hi, lo) = (first(&|b, a| b > a), first(&|b, a| b < a));
        let name = &t.headers[*c];
        if let Some((r, _)) = hi {
            out.push((format!("largest {name}"), r));
        }
        if i == 0 {
            if let Some((r, _)) = lo {
                out.push((format!("smallest {name}"), r));
            }
        }
    }
    // one entry per row: when the smallest is also the largest of another, keep the first label
    let mut seen = HashSet::new();
    out.retain(|(_, r)| seen.insert(*r));
    out.truncate(3);
    out
}

/// Pairs of numeric columns that move together (|Pearson r| of 0.9 or more), strongest first.
/// Redundant columns are worth knowing about before anyone fits a model to them. Identifier
/// columns are left out, because a column that rises with the row number correlates with
/// anything that drifts. A big table is sampled at an even stride.
fn correlation_note(t: &Table, info: &[ColInfo]) -> Option<String> {
    const MAX_COLS: usize = 12;
    const MAX_ROWS: usize = 100_000;
    let id = id_column(t, info);
    let stride = t.rows.len().div_ceil(MAX_ROWS).max(1);
    let mut cols: Vec<(usize, Vec<Option<f64>>)> = Vec::new();
    for c in 0..t.headers.len() {
        if cols.len() == MAX_COLS {
            break;
        }
        if !matches!(info[c].kind, Kind::Int | Kind::Float) || info[c].few_values || Some(c) == id {
            continue;
        }
        let all = numbers_of(t, c);
        if identifier_like(t, info, c, &all) {
            continue;
        }
        let sampled: Vec<Option<f64>> = (0..t.rows.len())
            .step_by(stride)
            .map(|r| parse_number(&t.rows[r][c]).map(|(f, _)| f))
            .collect();
        cols.push((c, sampled));
    }
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, (ca, a)) in cols.iter().enumerate() {
        for (cb, b) in &cols[i + 1..] {
            let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) =
                (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for (x, y) in a.iter().zip(b) {
                if let (Some(x), Some(y)) = (x, y) {
                    n += 1.0;
                    sa += x;
                    sb += y;
                    saa += x * x;
                    sbb += y * y;
                    sab += x * y;
                }
            }
            let (va, vb) = (saa - sa * sa / n, sbb - sb * sb / n);
            if n < 20.0 || va <= 0.0 || vb <= 0.0 {
                continue;
            }
            let r = (sab - sa * sb / n) / (va * vb).sqrt();
            if r.abs() >= 0.9 {
                pairs.push((r, *ca, *cb));
            }
        }
    }
    if pairs.is_empty() {
        return None;
    }
    pairs.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
    let shown: Vec<String> = pairs
        .iter()
        .take(3)
        .map(|(r, a, b)| format!("{}~{} {:.2}", t.headers[*a], t.headers[*b], r))
        .collect();
    let more = if pairs.len() > 3 {
        format!(", +{} more pairs", pairs.len() - 3)
    } else {
        String::new()
    };
    Some(format!(
        "correlated (Pearson r, |r| >= 0.9): {}{more}",
        shown.join(", ")
    ))
}

/// A number to four significant digits, which is all a group mean needs: `161.8`, `0.01235`.
fn sig4(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return "0".into();
    }
    if x.abs() < 1e-4 {
        return exp4(x);
    }
    let decimals = (3 - x.abs().log10().floor() as i32).clamp(0, 8) as usize;
    trim_zeros(format!("{x:.decimals$}"))
}

/// How much of a measure's variance a label column explains (eta squared), the label column, the
/// measure column, and the mean of the measure in each group.
type Breakdown = (f64, usize, usize, Vec<(String, f64)>);

/// Group means for the splits that matter most: a column with a few values (a region, a status, a
/// flag, an HTTP code) against each measure. "What is the mean amount for region eu?" is the
/// commonest question about a table, and these lines answer it without a second command.
///
/// The pair that varies most between groups is always shown; others only when the groups explain
/// at least a tenth of the variance (eta squared). At most three lines.
fn breakdown_notes(t: &Table, info: &[ColInfo]) -> Vec<String> {
    const MAX_MEASURES: usize = 6;
    const MAX_ROWS: usize = 200_000;
    const MIN_GROUP: usize = 5;
    const MAX_LINES: usize = 3;
    let n = t.rows.len();
    let id = id_column(t, info);
    let stride = n.div_ceil(MAX_ROWS).max(1);
    let sample: Vec<usize> = (0..n).step_by(stride).collect();
    // measures
    let mut measures: Vec<(usize, Vec<Option<f64>>)> = Vec::new();
    for c in 0..t.headers.len() {
        if measures.len() == MAX_MEASURES {
            break;
        }
        if !matches!(info[c].kind, Kind::Int | Kind::Float) || info[c].few_values || Some(c) == id {
            continue;
        }
        if identifier_like(t, info, c, &numbers_of(t, c)) {
            continue;
        }
        measures.push((
            c,
            sample
                .iter()
                .map(|&r| parse_number(&t.rows[r][c]).map(|(f, _)| f))
                .collect(),
        ));
    }
    // categories: a few values, in a column of labels, flags or codes
    let mut found: Vec<Breakdown> = Vec::new();
    #[allow(clippy::needless_range_loop)] // `c` indexes the table's columns and `info` alike
    for c in 0..t.headers.len() {
        let few = info[c].distinct.is_some_and(|d| (2..=8).contains(&d));
        let label = matches!(info[c].kind, Kind::Str | Kind::Bool)
            || (info[c].few_values && matches!(info[c].kind, Kind::Int));
        if !few
            || !label
            || Some(c) == id
            || measures.iter().any(|(m, _)| *m == c) && !info[c].few_values
        {
            continue;
        }
        let mut index: HashMap<&str, usize> = HashMap::new();
        let groups: Vec<Option<usize>> = sample
            .iter()
            .map(|&r| {
                let v = t.rows[r][c].as_str();
                if v.is_empty() || is_placeholder(v) {
                    return None;
                }
                let next = index.len();
                Some(*index.entry(v).or_insert(next))
            })
            .collect();
        let names: Vec<&str> = {
            let mut v = vec![""; index.len()];
            for (k, i) in &index {
                v[*i] = k;
            }
            v
        };
        for (m, vals) in &measures {
            if *m == c {
                continue;
            }
            let k = names.len();
            let (mut cnt, mut sum, mut sq) = (vec![0f64; k], vec![0f64; k], 0f64);
            let (mut total_n, mut total_sum) = (0f64, 0f64);
            for (g, v) in groups.iter().zip(vals) {
                if let (Some(g), Some(v)) = (g, v) {
                    cnt[*g] += 1.0;
                    sum[*g] += v;
                    sq += v * v;
                    total_n += 1.0;
                    total_sum += v;
                }
            }
            if total_n < 20.0 {
                continue;
            }
            let mean = total_sum / total_n;
            let ss_total = sq - total_n * mean * mean;
            let kept: Vec<usize> = (0..k).filter(|&g| cnt[g] >= MIN_GROUP as f64).collect();
            if kept.len() < 2 || ss_total <= 0.0 {
                continue;
            }
            let ss_between: f64 = kept
                .iter()
                .map(|&g| cnt[g] * (sum[g] / cnt[g] - mean).powi(2))
                .sum();
            let eta = ss_between / ss_total;
            let mut means: Vec<(String, f64)> = kept
                .iter()
                .map(|&g| (clip_label(names[g]), sum[g] / cnt[g]))
                .collect();
            means.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            found.push((eta, c, *m, means));
        }
    }
    // The pair people ask about first: the leftmost label column against the leftmost measure.
    // Then the splits that explain the most variance: the strongest of the rest, and any that
    // explain a quarter of it.
    let primary = found.iter().map(|f| (f.1, f.2)).min();
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut chosen: Vec<&Breakdown> = Vec::new();
    if let Some(p) = primary {
        chosen.extend(found.iter().filter(|f| (f.1, f.2) == p));
    }
    for (i, f) in found.iter().enumerate() {
        let strongest_rest = chosen.len() == 1 && f.0 >= 0.1;
        if chosen.len() < MAX_LINES
            && !chosen.iter().any(|c| (c.1, c.2) == (f.1, f.2))
            && (strongest_rest || f.0 >= 0.25 || i == 0 && f.0 >= 0.1)
        {
            chosen.push(f);
        }
    }
    chosen
        .iter()
        .map(|(_, c, m, means)| {
            let parts: Vec<String> = means
                .iter()
                .map(|(g, v)| format!("{g} {}", sig4(*v)))
                .collect();
            format!(
                "{} by {} (mean): {}",
                t.headers[*m],
                t.headers[*c],
                parts.join(", ")
            )
        })
        .collect()
}

/// A list of names (as `compact_names` writes it), cut when it is long, with the count of those
/// left out: `a, b, c, +12 more`.
fn clip_names(s: &str) -> String {
    const MAX: usize = 160;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let parts: Vec<&str> = s.split(", ").collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if out.len() + p.len() > MAX && i > 0 {
            return format!("{out}, +{} more", parts.len() - i);
        }
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(p);
    }
    out
}

fn clip_label(s: &str) -> String {
    let t: String = s.chars().take(24).collect();
    if s.chars().count() > 24 {
        format!("{t}…")
    } else {
        t
    }
}

/// Rows that repeat an earlier row, which most data sets do not mean to have.
fn duplicate_note(t: &Table) -> Option<String> {
    let mut first: HashMap<&Vec<String>, usize> = HashMap::with_capacity(t.rows.len());
    let mut repeats = 0usize;
    let mut example: Option<(usize, usize)> = None;
    for (r, row) in t.rows.iter().enumerate() {
        match first.get(row) {
            Some(&q) => {
                repeats += 1;
                example.get_or_insert((r, q));
            }
            None => {
                first.insert(row, r);
            }
        }
    }
    example.map(|(r, q)| {
        format!(
            "duplicates: {repeats} row{} repeat{} an earlier row (first: row {r} = row {q})",
            if repeats == 1 { "" } else { "s" },
            if repeats == 1 { "s" } else { "" }
        )
    })
}

fn summary(
    t: &Table,
    info: &[ColInfo],
    ctx: &Ctx,
    p: &Profile,
    wanted: &[(String, usize)],
) -> String {
    let n = t.rows.len();
    let cols = t.headers.len();
    let mut out = String::new();
    out.push_str(&format!(
        "[dtk] {}: {} rows x {} cols, {} (statistics over all rows; the full data is on disk)\n",
        t.format.name(),
        n,
        cols,
        human_bytes(ctx.bytes)
    ));
    if let Some(p) = &ctx.csv_path {
        out.push_str(&format!("data: {} (csv, header row)\n", shell_quote(p)));
    }
    if let Some(p) = &ctx.raw_path {
        out.push_str(&format!("raw: {} ({})\n", shell_quote(p), t.format.name()));
    }
    // Every column is named. Statistics go to the columns that vary; a column that is the same in
    // every row, or different in every row (an id), costs a name and not a line. A table with
    // dozens of varying columns (features) gets statistics for a dozen of them, and the model
    // asks for the others with `dtk stats`.
    let class: Vec<ColClass> = (0..cols).map(|c| classify(t, info, c)).collect();
    let varying: Vec<usize> = (0..cols)
        .filter(|&c| class[c] == ColClass::Informative)
        .collect();
    let copies = aliases(t, info, &varying);
    let informative: Vec<usize> = varying
        .iter()
        .copied()
        .filter(|c| !copies.iter().any(|(a, _)| a == c))
        .collect();
    let k = if informative.len() > VERY_WIDE_COLS {
        p.stat_cols.min(12)
    } else {
        p.stat_cols.min(informative.len())
    };
    let stat_cols: Vec<usize> = informative.iter().take(k).copied().collect();
    out.push_str(&format!("cols: {}\n", compact_cols(&t.headers, info)));
    for &c in &stat_cols {
        out.push_str(&format!("{}: {}\n", t.headers[c], info[c].summary));
    }
    if informative.len() > k {
        let rest: Vec<&str> = informative[k..]
            .iter()
            .map(|&c| t.headers[c].as_str())
            .collect();
        out.push_str(&format!(
            "no statistics shown for {} more columns ({}): {} stats {} NAME...\n",
            rest.len(),
            clip_names(&compact_names(&rest)),
            command_name(),
            data_ref(ctx)
        ));
    }
    // Columns that say what an earlier column says: one line per original.
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for &(copy, original) in &copies {
        match groups.iter_mut().find(|g| g.0 == original) {
            Some(g) => g.1.push(copy),
            None => groups.push((original, vec![copy])),
        }
    }
    for (original, copy_cols) in groups.iter().take(8) {
        let names: Vec<&str> = copy_cols.iter().map(|&c| t.headers[c].as_str()).collect();
        out.push_str(&format!(
            "same groups as {}: {}\n",
            t.headers[*original],
            clip_names(&compact_names(&names))
        ));
    }
    let constants: Vec<usize> = (0..cols)
        .filter(|&c| class[c] == ColClass::Constant)
        .collect();
    if !constants.is_empty() {
        let shown: Vec<String> = constants
            .iter()
            .take(16)
            .map(|&c| {
                let v = info[c]
                    .stats
                    .top
                    .first()
                    .map_or("(empty)", |x| x.0.as_str());
                format!("{}={}", t.headers[c], clip_label(v))
            })
            .collect();
        out.push_str(&format!("same in every row: {}\n", shown.join(", ")));
        // The values of the first sixteen are shown. The rest are named, so that none is lost.
        if constants.len() > 16 {
            let rest: Vec<&str> = constants[16..]
                .iter()
                .map(|&c| t.headers[c].as_str())
                .collect();
            out.push_str(&format!(
                "  also the same in every row: {}\n",
                clip_names(&compact_names(&rest))
            ));
        }
    }
    let uniques: Vec<&str> = (0..cols)
        .filter(|&c| class[c] == ColClass::Unique)
        .map(|c| t.headers[c].as_str())
        .collect();
    if !uniques.is_empty() {
        out.push_str(&format!(
            "different in every row (ids, urls): {}\n",
            clip_names(&compact_names(&uniques))
        ));
        // one example of the first two, so that their shape is known
        let examples: Vec<String> = (0..cols)
            .filter(|&c| class[c] == ColClass::Unique)
            .take(2)
            .map(|c| format!("{} like {}", t.headers[c], clip_label(&t.rows[0][c])))
            .collect();
        out.push_str(&format!("  ({})\n", examples.join("; ")));
    }
    // The sample rows show the columns above that vary, plus the one that names a row and the
    // level and message of a log. If nothing varies, they show the first columns.
    let mut shown: Vec<usize> = stat_cols.clone();
    if shown.is_empty() {
        shown = (0..cols.min(p.stat_cols)).collect();
    } else {
        let must = id_column(t, info).into_iter().chain(
            t.headers
                .iter()
                .enumerate()
                .filter(|(_, h)| {
                    LEVEL_NAMES.contains(&h.to_lowercase().as_str()) || h.as_str() == "msg"
                })
                .map(|(i, _)| i),
        );
        shown.extend(must);
        // The first column is what the reader sees first in a row, and it is checked against.
        if !constants.contains(&0)
            && !shown.contains(&0)
            && cols > 0
            && class[0] != ColClass::Unique
        {
            shown.push(0);
        }
        shown.sort_unstable();
        shown.dedup();
    }
    let all_in_order = shown.len() == cols;
    if let Some(note) = duplicate_note(t) {
        out.push_str(&note);
        out.push('\n');
    }
    if let Some(note) = correlation_note(t, info) {
        out.push_str(&note);
        out.push('\n');
    }
    if cols <= VERY_WIDE_COLS {
        for line in breakdown_notes(t, info) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let clip = |rows: &[&Vec<String>]| -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| shown.iter().map(|&c| r[c].clone()).collect())
            .collect()
    };
    let show = |rows: &[&Vec<String>]| -> String {
        let clipped = clip(rows);
        let refs: Vec<&Vec<String>> = clipped.iter().collect();
        write_csv(None, &refs, Some(p.max_cell))
    };
    // The rows of a log that say what went wrong are read for their message, so they are cut
    // later than sample rows are, and each says which row of the log it is.
    let show_problems = |idx: &[usize]| -> String {
        idx.iter()
            .map(|&i| {
                let clipped = clip(&[&t.rows[i]]);
                let refs: Vec<&Vec<String>> = clipped.iter().collect();
                format!(
                    "row {}: {}",
                    i + 1,
                    write_csv(None, &refs, Some(PROBLEM_CELL)).trim_end()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Wide rows cost more, so fewer of them are shown: about 1,400 characters of sample rows.
    let sample_len: usize = t
        .rows
        .iter()
        .take(20)
        .map(|r| {
            shown
                .iter()
                .map(|&c| r[c].chars().count().min(p.max_cell) + 1)
                .sum::<usize>()
        })
        .sum::<usize>()
        / t.rows.len().clamp(1, 20);
    let rows_wanted = (1400 / sample_len.max(1)).clamp(3, p.head + p.tail);
    let head_n = (rows_wanted * 2).div_ceil(3).clamp(2, p.head);
    let tail_n = (rows_wanted - head_n.min(rows_wanted)).clamp(1, p.tail);
    let head: Vec<&Vec<String>> = t.rows.iter().take(head_n).collect();
    let which = if all_in_order {
        "columns in order above".to_string()
    } else {
        let names: Vec<&str> = shown.iter().map(|&c| t.headers[c].as_str()).collect();
        format!("columns: {}", clip_names(&compact_names(&names)))
    };
    out.push_str(&format!(
        "head {} ({which}):\n{}\n",
        head.len(),
        show(&head)
    ));
    let tail_from = n.saturating_sub(tail_n).max(head_n.min(n));
    let tail: Vec<&Vec<String>> = t.rows.iter().skip(tail_from).collect();
    out.push_str(&format!("tail {}:\n{}\n", tail.len(), show(&tail)));
    // The rows that the notes above point at, in full: the largest and smallest of the first
    // measure and the largest of the second. They say more than the first and last rows do.
    let shown_rows = |r: usize| r < head_n.min(n) || r >= tail_from;
    let notable = notable_rows(t, info);
    let picked: Vec<(String, usize)> = notable
        .into_iter()
        .filter(|(_, r)| !shown_rows(*r))
        .collect();
    if !picked.is_empty() {
        let labels: Vec<&str> = picked.iter().map(|(l, _)| l.as_str()).collect();
        let rows: Vec<&Vec<String>> = picked.iter().map(|(_, r)| &t.rows[*r]).collect();
        out.push_str(&format!(
            "extremes ({}):\n{}\n",
            labels.join("; "),
            show(&rows)
        ));
    }
    // The rows that hold a word of the request as a whole cell, when they are not shown yet.
    let asked: Vec<&(String, usize)> = wanted
        .iter()
        .filter(|(_, r)| !shown_rows(*r) && !picked.iter().any(|(_, p)| p == r))
        .collect();
    if !asked.is_empty() {
        let mut labels: Vec<&str> = Vec::new();
        for (l, _) in &asked {
            if !labels.contains(&l.as_str()) {
                labels.push(l.as_str());
            }
        }
        let rows: Vec<&Vec<String>> = asked.iter().map(|(_, r)| &t.rows[*r]).collect();
        out.push_str(&format!(
            "matches for words in your request ({}):\n{}\n",
            labels.join("; "),
            show(&rows)
        ));
    }
    if let Some(li) = t
        .headers
        .iter()
        .position(|h| LEVEL_NAMES.contains(&h.to_lowercase().as_str()))
    {
        let bad: Vec<&Vec<String>> = t
            .rows
            .iter()
            .filter(|r| BAD_LEVELS.contains(&r[li].to_lowercase().as_str()))
            .collect();
        if !bad.is_empty() && t.format == Format::TextLog {
            // In a text log the lines just before an error say what failed. Errors come first,
            // each with the lines above it; a warning stands alone.
            let is_warn =
                |r: &Vec<String>| matches!(r[li].to_lowercase().as_str(), "warn" | "warning");
            let (mut errors, mut warns) = (Vec::new(), Vec::new());
            for (i, r) in t.rows.iter().enumerate() {
                if BAD_LEVELS.contains(&r[li].to_lowercase().as_str()) {
                    if is_warn(r) { &mut warns } else { &mut errors }.push(i);
                }
            }
            let cap = p.important + 8;
            let mut keep = std::collections::BTreeSet::new();
            // Many errors are usually the same error again and again. Group them by the shape
            // of their message, say how many of each, and show the first of each with its context.
            let msg_col = t.headers.iter().position(|h| h == "msg");
            let mut shape_lines = String::new();
            let mut firsts: Vec<usize> = errors.clone();
            if let (true, Some(mc)) = (errors.len() > 5, msg_col) {
                let mut groups: Vec<(String, usize, usize)> = Vec::new(); // shape, count, first row
                for &i in &errors {
                    let first_line = t.rows[i][mc].lines().next().unwrap_or("");
                    let shape: String = template(first_line).chars().take(110).collect();
                    match groups.iter_mut().find(|g| g.0 == shape) {
                        Some(g) => g.1 += 1,
                        None => groups.push((shape, 1, i)),
                    }
                }
                groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
                shape_lines.push_str(&format!("error shapes ({}):\n", groups.len()));
                for (shape, n, at) in groups.iter().take(5) {
                    shape_lines.push_str(&format!(
                        "  {n}x {shape}   (first at row {}, {})\n",
                        at + 1,
                        t.rows[*at][0]
                    ));
                }
                if groups.len() > 5 {
                    shape_lines.push_str(&format!("  ... {} more shapes\n", groups.len() - 5));
                }
                firsts = groups.iter().take(5).map(|g| g.2).collect();
                firsts.sort_unstable();
            }
            for &i in &firsts {
                if keep.len() >= cap {
                    break;
                }
                keep.extend(i.saturating_sub(CONTEXT_LINES)..=i);
            }
            for &i in &warns {
                if keep.len() >= cap {
                    break;
                }
                keep.insert(i);
            }
            let keep: Vec<usize> = keep.into_iter().collect();
            out.push_str(&format!(
                "{} error{}, {} warning{} ({}).\n{}showing {} lines, each {}error with the {} lines before it:\n{}\n",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" },
                warns.len(),
                if warns.len() == 1 { "" } else { "s" },
                t.headers[li],
                shape_lines,
                keep.len(),
                if errors.len() > 5 && msg_col.is_some() { "first " } else { "" },
                CONTEXT_LINES,
                show_problems(&keep)
            ));
        } else if !bad.is_empty() {
            let shown: Vec<&Vec<String>> = bad.iter().take(p.important).copied().collect();
            out.push_str(&format!(
                "{} rows with {}=warn/error: showing {} of {}:\n{}\n",
                bad.len(),
                t.headers[li],
                shown.len(),
                bad.len(),
                show(&shown)
            ));
        }
    }
    for note in &t.notes {
        out.push_str(&format!("note: {}\n", note));
    }
    out.push_str(&drill(t, info, ctx));
    out.trim_end().to_string()
}

/// How the summaries call dtk. `dtk` when that works in the model's shell; the full path when
/// the folder dtk is installed in is not on the PATH.
static COMMAND: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_command_name(name: &str) {
    let _ = COMMAND.set(name.to_string());
}

pub fn command_name() -> &'static str {
    COMMAND.get().map_or("dtk", String::as_str)
}

/// How to name a data file in a command: the short id when it is one of dtk's own files
/// (`5fa6aeae75bd` for `.../dtk/store/5fa6aeae75bd.csv`), else the quoted path.
pub(crate) fn file_ref(path: &str) -> String {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let in_store = p
        .parent()
        .and_then(|d| d.file_name())
        .is_some_and(|n| n == "store");
    if in_store && stem.len() == 12 && stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        stem.to_string()
    } else {
        shell_quote(path)
    }
}

/// The commands that read more of the data behind a summary, with real column names and values
/// as examples so that the model can copy them.
fn drill(t: &Table, info: &[ColInfo], ctx: &Ctx) -> String {
    let Some(csv) = &ctx.csv_path else {
        return String::new();
    };
    let (dtk, f) = (command_name(), file_ref(csv));
    let word = |x: &str| {
        if !x.is_empty()
            && x.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            x.to_string()
        } else {
            format!("'{}'", x.replace('\'', "'\\''"))
        }
    };
    let label = info
        .iter()
        .position(|i| i.kind == Kind::Str && i.distinct.is_some_and(|d| d > 1 && d <= 50))
        .or_else(|| info.iter().position(|i| i.kind == Kind::Str));
    let id = id_column(t, info);
    let measure = (0..t.headers.len()).find(|&c| {
        matches!(info[c].kind, Kind::Int | Kind::Float)
            && !info[c].few_values
            && Some(c) != id
            && !id_like_name(&t.headers[c])
    });
    // Three examples with real names are enough to copy; the rest is one `--help` away.
    let mut cmds: Vec<String> = Vec::new();
    if let Some((l, top)) = label.and_then(|l| Some((l, info[l].stats.top.first()?))) {
        cmds.push(format!(
            "{dtk} where {f} {} = {}",
            word(&t.headers[l]),
            word(&top.0)
        ));
    }
    if let Some(m) = measure {
        cmds.push(format!("{dtk} top {f} {} 10", word(&t.headers[m])));
    }
    if let (Some(l), Some(m)) = (label, measure) {
        cmds.push(format!(
            "{dtk} group {f} {} {}",
            word(&t.headers[l]),
            word(&t.headers[m])
        ));
    }
    if cmds.is_empty() {
        cmds.push(format!("{dtk} rows {f} 100 20"));
    }
    cmds.push(format!("(also rows, count, stats, get: {dtk} rows --help)"));
    format!("drill: {}\n", cmds.join("  |  "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{detect, tests as fx};

    fn ctx() -> Ctx {
        Ctx {
            bytes: 100_000,
            csv_path: Some("/tmp/x.csv".into()),
            raw_path: Some("/tmp/x.jsonl".into()),
        }
    }

    #[test]
    fn numbers_keep_four_significant_digits_below_a_tenth() {
        assert_eq!(fnum(0.0), "0");
        assert_eq!(fnum(1234.0), "1234");
        assert_eq!(fnum(100.29971), "100.2997");
        assert_eq!(fnum(0.5), "0.5");
        assert_eq!(fnum(0.0875123), "0.08751");
        assert_eq!(fnum(0.00031234), "0.0003123");
        assert_eq!(fnum(2.96e-8), "2.96e-8");
        assert_eq!(fnum(-1.5e-7), "-1.5e-7");
        assert_eq!(fnum(-0.00002), "-2e-5");
        assert_eq!(sig4(2.96e-8), "2.96e-8");
        assert_eq!(sig4(0.012345), "0.01235");
    }

    #[test]
    fn a_column_of_tiny_values_is_not_shown_as_zero() {
        let mut text = String::from("id,rate,loss\n");
        for i in 0..80 {
            text.push_str(&format!(
                "{i},{:.3e},{:.6}\n",
                (i + 1) as f64 * 2.5e-8,
                (i + 1) as f64 * 3e-6
            ));
        }
        let t = detect(&text).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(!r.lossless);
        // The smallest and the largest value are the cells as written.
        assert!(
            r.text.contains("rate: 2.500e-8..2.000e-6 mean "),
            "{}",
            r.text
        );
        assert!(
            r.text.contains("loss: 0.000003..0.00024 mean "),
            "{}",
            r.text
        );
        assert!(!r.text.contains("mean 0 "), "{}", r.text);
    }

    #[test]
    fn small_table_is_lossless_csv() {
        let t = detect(&fx::jsonl_text(30)).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(r.lossless);
        assert!(r.text.starts_with("[dtk] jsonl as csv: 30 rows x 4 cols"));
        assert!(r.text.contains("ts,level,msg,ms"));
        assert_eq!(r.text.lines().count(), 1 + 31);
    }

    #[test]
    fn big_table_gets_summary_with_types_and_drill() {
        let t = detect(&fx::csv_text(500)).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(!r.lossless);
        assert!(r.text.contains("500 rows x 5 cols"));
        assert!(r
            .text
            .contains("cols: id:int name:str region:str amount:float ok:bool"));
        assert!(r.text.contains("id: 1000..1499"));
        assert!(r.text.contains("region: 3 distinct: "));
        assert!(
            r.text.contains("drill: dtk where /tmp/x.csv name = user_0  |  dtk top /tmp/x.csv amount 10  |  dtk group /tmp/x.csv name amount"),
            "{}",
            r.text
        );
        assert!(
            r.text
                .contains("(also rows, count, stats, get: dtk rows --help)"),
            "{}",
            r.text
        );
        assert!(est_tokens(&r.text) <= 2500);
    }

    #[test]
    fn warn_and_error_rows_are_kept() {
        let t = detect(&fx::jsonl_text(400)).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(r
            .text
            .contains("rows with level=warn/error: showing 20 of 200"));
        assert!(
            r.text.contains("dtk where /tmp/x.csv level = info"),
            "{}",
            r.text
        );
    }

    #[test]
    fn few_valued_numbers_show_counts() {
        let mut s = String::from("code,ms\n");
        for i in 0..200 {
            s.push_str(&format!("{},{}\n", [200, 200, 404, 500][i % 4], i * 3));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert!(
            info[0].summary.starts_with("3 distinct: 200 100"),
            "{}",
            info[0].summary
        );
        assert!(info[1].summary.contains("mean"), "{}", info[1].summary);
    }

    #[test]
    fn leading_zero_ids_stay_strings() {
        let mut s = String::from("zip,city\n");
        for i in 0..100 {
            s.push_str(&format!("{:05},town{}\n", i, i % 3));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert_eq!(info[0].kind, Kind::Str);
    }

    #[test]
    fn summary_is_much_smaller_than_input() {
        let text = fx::csv_text(5000);
        let t = detect(&text).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(est_tokens(&r.text) * 20 < est_tokens(&text));
    }

    #[test]
    fn template_replaces_numbers_and_ids() {
        assert_eq!(template("request 4821 took 55ms"), "request # took #ms");
        assert_eq!(template("user 9f8e7d6c5b4a done"), "user <hex> done");
        assert_eq!(
            template("id 123e4567-e89b-12d3-a456-426614174000 ok"),
            "id <uuid> ok"
        );
        assert_eq!(template("v1.2.3 build abc"), "v#.#.# build abc");
        assert_eq!(template("plain text"), "plain text");
        assert_eq!(
            template("deadbeef"),
            "deadbeef",
            "letters only is a word, not an id"
        );
    }

    #[test]
    fn many_distinct_messages_collapse_to_their_templates_with_exact_counts() {
        let mut s = String::from("level,msg\n");
        for i in 0..2000 {
            s.push_str(&format!("info,request {} took {}ms\n", i, i * 3 % 997));
        }
        for i in 0..700 {
            s.push_str(&format!("warn,cache miss for key a{:07x}9\n", i * 7919));
        }
        for i in 0..300 {
            s.push_str(&format!("error,worker {} crashed\n", i));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        let c = &info[1];
        assert_eq!(
            c.stats.templates[0],
            ("request # took #ms".to_string(), 2000)
        );
        assert_eq!(
            c.stats.templates[1],
            ("cache miss for key <hex>".to_string(), 700)
        );
        assert_eq!(c.stats.templates[2], ("worker # crashed".to_string(), 300));
        assert!(
            c.summary.starts_with("3000 distinct, 3 templates: request # took #ms (2000); cache miss for key <hex> (700); worker # crashed (300)"),
            "{}",
            c.summary
        );
    }

    #[test]
    fn templates_survive_more_than_5000_distinct_messages() {
        let mut s = String::from("level,msg\n");
        for i in 0..9000 {
            s.push_str(&format!("info,request {i} took {}ms\n", i * 7 % 991));
        }
        for i in 0..3000 {
            s.push_str(&format!("warn,cache miss for key a{:07x}9\n", i * 7919));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        let c = &info[1];
        assert!(
            c.summary.starts_with(">5000 distinct, 2 templates: request # took #ms (9000); cache miss for key <hex> (3000)"),
            "{}",
            c.summary
        );
        assert_eq!(
            c.stats.templates[0],
            ("request # took #ms".to_string(), 9000)
        );
    }

    #[test]
    fn repeated_errors_are_grouped_by_shape_in_a_text_log() {
        let mut s = String::new();
        for i in 0..300 {
            let msg = match i {
                n if n % 10 == 5 => format!("error: connection {n} refused by 10.0.0.{}", n % 9),
                n if n % 50 == 7 => format!("FAILED test_{n}"),
                n => format!("step {n} ok"),
            };
            s.push_str(&format!(
                "2026-11-02T10:{:02}:{:02}Z {msg}\n",
                i / 60,
                i % 60
            ));
        }
        let t = detect(&s).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(r.text.contains("error shapes (2):"), "{}", r.text);
        assert!(
            r.text
                .contains("30x error: connection # refused by #.#.#.#"),
            "{}",
            r.text
        );
        assert!(r.text.contains("6x FAILED test_#"), "{}", r.text);
        assert!(
            r.text
                .contains("each first error with the 4 lines before it"),
            "{}",
            r.text
        );
        // the 30 refused-connection errors are not all listed
        assert!(r.text.matches("refused by").count() < 8, "{}", r.text);
    }

    #[test]
    fn reads_numbers_as_people_write_them() {
        for (text, value, fmt) in [
            ("12.5", 12.5, NumFmt::Plain),
            ("-7", -7.0, NumFmt::Plain),
            ("1e3", 1000.0, NumFmt::Plain),
            ("1,234", 1234.0, NumFmt::Thousands),
            ("1,234,567.89", 1234567.89, NumFmt::Thousands),
            ("$1,234.56", 1234.56, NumFmt::Currency),
            ("€5", 5.0, NumFmt::Currency),
            ("-$5.25", -5.25, NumFmt::Currency),
            ("$-5.25", -5.25, NumFmt::Currency),
            ("45.3%", 45.3, NumFmt::Percent),
            ("(12.50)", -12.5, NumFmt::Plain),
            ("(1,200)", -1200.0, NumFmt::Thousands),
        ] {
            assert_eq!(parse_number(text), Some((value, fmt)), "{text}");
        }
        for text in [
            "", "abc", "007", "02134", "1,23", "12,34", "1,2345", ",123", "1,", "$", "%", "$5%",
            "1.2.3", "1,5", "12abc", "NaN", "inf", "5 %", "1,234,56", "--5", "$$5", "0123.5",
        ] {
            assert_eq!(parse_number(text), None, "{text:?} is not a number");
        }
    }

    #[test]
    fn a_column_of_numbers_with_placeholders_is_a_column_of_numbers() {
        let mut s = String::from("id,age,city\n");
        for i in 0..60 {
            let age = match i % 10 {
                3 => "N/A".to_string(),
                7 => "-".to_string(),
                _ => (20 + i % 40).to_string(),
            };
            s.push_str(&format!("{i},{age},c{}\n", i % 5));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        let age = &info[1];
        assert_eq!(age.kind, Kind::Int);
        assert_eq!(age.stats.placeholders, 12);
        assert!(age.summary.starts_with("20..59 mean "), "{}", age.summary);
        assert!(
            age.summary
                .contains("12 placeholders excluded (- 6, N/A 6)"),
            "{}",
            age.summary
        );
        assert_eq!(age.stats.nonempty, 48);
    }

    #[test]
    fn placeholders_in_a_column_of_words_are_just_words() {
        let mut s = String::from("id,status\n");
        for i in 0..60 {
            s.push_str(&format!("{i},{}\n", ["ok", "fail", "-", "ok"][i % 4]));
        }
        let t = detect(&s).unwrap();
        let st = &analyze(&t)[1];
        assert_eq!(st.kind, Kind::Str);
        assert_eq!(st.stats.placeholders, 0);
        assert!(st.summary.contains("- 15"), "{}", st.summary);
    }

    #[test]
    fn a_column_that_is_mostly_placeholders_is_not_numbers() {
        let mut s = String::from("id,score\n");
        for i in 0..60 {
            s.push_str(&format!("{i},{}\n", if i % 5 == 0 { "7" } else { "N/A" }));
        }
        let t = detect(&s).unwrap();
        assert_eq!(analyze(&t)[1].kind, Kind::Str);
    }

    #[test]
    fn formatted_numbers_get_statistics_and_say_how_they_are_written() {
        let mut s = String::from("id,price,share,qty\n");
        for i in 0..40 {
            s.push_str(&format!(
                "{i},\"${},{:03}.50\",{}.5%,\"{},000\"\n",
                1 + i % 3,
                i * 7 % 1000,
                i % 100,
                1 + i % 9
            ));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert_eq!(info[1].kind, Kind::Float);
        assert_eq!(info[1].stats.written_as, Some("currency"));
        assert!(
            info[1].summary.contains("written with currency"),
            "{}",
            info[1].summary
        );
        assert_eq!(info[2].stats.written_as, Some("percent"));
        assert_eq!(info[3].kind, Kind::Int);
        assert_eq!(info[3].stats.written_as, Some("thousands separators"));
        assert_eq!(info[3].stats.min, Some(1000.0));
        assert_eq!(info[3].stats.max, Some(9000.0));
    }

    #[test]
    fn two_ways_of_writing_numbers_in_one_column_are_not_numbers() {
        let mut s = String::from("id,v\n");
        for i in 0..40 {
            s.push_str(&format!(
                "{i},{}\n",
                if i % 2 == 0 { "$5.00" } else { "45%" }
            ));
        }
        let t = detect(&s).unwrap();
        assert_eq!(analyze(&t)[1].kind, Kind::Str);
        // a percent column is all percents: 0.45 and 45% would mean different things
        let mut s = String::from("id,v\n");
        for i in 0..40 {
            s.push_str(&format!(
                "{i},{}\n",
                if i % 2 == 0 { "0.45" } else { "45%" }
            ));
        }
        let t = detect(&s).unwrap();
        assert_eq!(analyze(&t)[1].kind, Kind::Str);
    }

    #[test]
    fn dates_with_slashes_are_dates_when_the_data_says_which_order() {
        let mk = |f: &dyn Fn(usize) -> String| {
            let mut s = String::from("id,when\n");
            for i in 0..40 {
                s.push_str(&format!("{i},{}\n", f(i)));
            }
            detect(&s).unwrap()
        };
        // a second part above 12: month first
        let t = mk(&|i| format!("{:02}/{:02}/2025", i % 12 + 1, i % 28 + 1));
        let c = &analyze(&t)[1];
        assert_eq!(c.kind, Kind::Time);
        assert!(
            c.summary.starts_with("2025-01-01..2025-12-"),
            "{}",
            c.summary
        );
        assert!(
            c.summary.contains("dates written MM/DD/YYYY"),
            "{}",
            c.summary
        );
        // a first part above 12: day first
        let t = mk(&|i| format!("{:02}.{:02}.2025", i % 28 + 1, i % 12 + 1));
        let c = &analyze(&t)[1];
        assert_eq!(c.kind, Kind::Time);
        assert!(
            c.summary.contains("dates written DD/MM/YYYY"),
            "{}",
            c.summary
        );
        // every part 12 or less: the data does not say, so it stays text
        let t = mk(&|i| format!("{:02}/{:02}/2025", i % 12 + 1, (i / 4) % 12 + 1));
        assert_eq!(analyze(&t)[1].kind, Kind::Str);
        // both orders in one column: not a date column
        let t = mk(&|i| {
            if i % 2 == 0 {
                "25/01/2025".into()
            } else {
                "01/25/2025".into()
            }
        });
        assert_eq!(analyze(&t)[1].kind, Kind::Str);
        // a placeholder does not spoil it
        let t = mk(&|i| {
            if i % 9 == 0 {
                "N/A".into()
            } else {
                format!("{:02}/{:02}/2025", i % 12 + 1, i % 18 + 13)
            }
        });
        let c = &analyze(&t)[1];
        assert_eq!(c.kind, Kind::Time, "{}", c.summary);
        assert_eq!(c.stats.placeholders, 5);
    }

    #[test]
    fn reads_iso_times_to_seconds() {
        assert_eq!(iso_seconds("1970-01-01"), Some(0.0));
        assert_eq!(iso_seconds("1970-01-02 00:00"), Some(86_400.0));
        assert_eq!(iso_seconds("2000-03-01T00:00:00Z"), Some(951_868_800.0));
        assert_eq!(
            iso_seconds("2026-01-01T00:00:00.250Z"),
            Some(1_767_225_600.25)
        );
        assert_eq!(
            iso_seconds("2026-01-01T02:00:00+02:00"),
            Some(1_767_225_600.0)
        );
        assert_eq!(
            iso_seconds("2026-01-01T00:00:00-0130"),
            Some(1_767_225_600.0 + 5400.0)
        );
        for bad in [
            "",
            "2026-13-01",
            "2026/01/01",
            "2026-01-01X10:00",
            "20260101T0000",
            "2026-01-01 10",
            "héllo wörld",
            "2026-01-01T10:00:00+",
        ] {
            assert_eq!(iso_seconds(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn writes_durations_for_people() {
        for (secs, want) in [
            (0.5, "0.5s"),
            (45.0, "45s"),
            (90.0, "1min30s"),
            (37.0 * 60.0, "37min"),
            (5400.0, "1h30min"),
            (86_400.0, "1d"),
            (2.0 * 86_400.0 + 3.0 * 3600.0 + 240.0, "2d3h"),
            (7.0 * 86_400.0, "7d"),
        ] {
            assert_eq!(duration(secs), want, "{secs}");
        }
    }

    fn timed(f: &dyn Fn(usize) -> String) -> Table {
        let mut s = String::from("id,ts,v\n");
        for i in 0..60 {
            s.push_str(&format!("{i},{},{}\n", f(i), i * 3 % 17));
        }
        detect(&s).unwrap()
    }

    #[test]
    fn a_time_column_says_whether_it_is_sorted_and_evenly_spaced() {
        let at = |i: usize| 1_767_225_600 + i as i64 * 37 * 60;
        let iso = |secs: i64| {
            let (d, r) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
            // civil date from days (inverse of days_from_civil, enough for 2026)
            let z = d + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = yoe + era * 400 + i64::from(m <= 2);
            format!(
                "{y:04}-{m:02}-{day:02} {:02}:{:02}:{:02}",
                r / 3600,
                r % 3600 / 60,
                r % 60
            )
        };
        let t = timed(&|i| iso(at(i)));
        let c = &analyze(&t)[1];
        assert!(
            c.summary.contains("sorted oldest first, every 37min"),
            "{}",
            c.summary
        );
        assert!(!c.summary.contains("longest gap"), "{}", c.summary);
        // newest first
        let t = timed(&|i| iso(at(59 - i)));
        assert!(analyze(&t)[1]
            .summary
            .contains("sorted newest first, every 37min"));
        // a two-day hole after row 29
        let t = timed(&|i| iso(at(i) + if i >= 30 { 2 * 86_400 } else { 0 }));
        let c = &analyze(&t)[1];
        assert!(
            c.summary.contains("longest gap 2d37min after row 29")
                || c.summary.contains("longest gap 2d1h after row 29"),
            "{}",
            c.summary
        );
        // uneven but in order
        let t = timed(&|i| iso(at(0) + (i * i) as i64 * 60));
        assert!(
            analyze(&t)[1]
                .summary
                .contains("sorted oldest first, typical step"),
            "{}",
            analyze(&t)[1].summary
        );
        // out of order
        let t = timed(&|i| iso(at((i * 7) % 60)));
        assert!(analyze(&t)[1].summary.contains("not in time order"));
        // dates only
        let t = timed(&|i| format!("2026-{:02}-{:02}", i / 28 + 1, i % 28 + 1));
        assert!(
            analyze(&t)[1].summary.contains("every 1d"),
            "{}",
            analyze(&t)[1].summary
        );
    }

    #[test]
    fn strongly_correlated_columns_are_named() {
        let mut s = String::from("row,x,y,z,w,v\n");
        for i in 0..200usize {
            // x rises and falls; y = 2x + a little noise; w = -x; z and v are unrelated
            let x = ((i * 37) % 101) as f64;
            let y = 2.0 * x + (i % 3) as f64;
            let z = ((i * 53) % 97) as f64;
            let v = ((i * 29 + 7) % 89) as f64;
            s.push_str(&format!("{},{x},{y},{z},{},{v}\n", i % 7, -x));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        let note = correlation_note(&t, &info).expect("note");
        assert!(note.contains("x~y 1.00"), "{note}");
        assert!(note.contains("x~w -1.00"), "{note}");
        assert!(!note.contains('z') && !note.contains('v'), "{note}");
        // nothing correlated: no note
        let mut s = String::from("a,b,c\n");
        for i in 0..200usize {
            s.push_str(&format!(
                "{},{},{}\n",
                (i * 37) % 101,
                (i * 53) % 97,
                (i * 29 + 7) % 89
            ));
        }
        let t = detect(&s).unwrap();
        assert!(correlation_note(&t, &analyze(&t)).is_none());
        // an identifier that rises with the row is not "correlated" with a drifting column
        let mut s = String::from("order_id,drift,other\n");
        for i in 0..200usize {
            s.push_str(&format!(
                "{},{},{}\n",
                1000 + i,
                i * 3 + (i * 7) % 5,
                (i * 53) % 97
            ));
        }
        let t = detect(&s).unwrap();
        assert!(correlation_note(&t, &analyze(&t)).is_none());
    }

    #[test]
    fn writes_four_significant_digits() {
        for (x, want) in [
            (161.7586, "161.8"),
            (101.5044, "101.5"),
            (5.7234, "5.723"),
            (0.0123456, "0.01235"),
            (12345.678, "12346"),
            (1234.5, "1234"),
            (100.0, "100"),
            (0.0, "0"),
            (-3.3333, "-3.333"),
        ] {
            assert_eq!(sig4(x), want, "{x}");
        }
    }

    #[test]
    fn group_means_are_in_the_summary_for_the_split_that_matters() {
        let mut s = String::from("order,region,tier,amount,wait\n");
        for i in 0..240usize {
            let region = ["us", "eu", "ap"][i % 3];
            let tier = ["free", "pro"][(i / 3) % 2];
            // amount depends on the region; wait depends on nothing
            let amount = match region {
                "us" => 100.0,
                "eu" => 200.0,
                _ => 300.0,
            } + (i % 7) as f64;
            let wait = ((i * 37) % 11) as f64;
            s.push_str(&format!("{},{region},{tier},{amount},{wait}\n", 5000 + i));
        }
        let t = detect(&s).unwrap();
        let lines = breakdown_notes(&t, &analyze(&t));
        assert!(
            lines[0].starts_with("amount by region (mean): ap 30"),
            "the strongest split comes first: {lines:?}"
        );
        assert!(
            lines[0].contains("eu 20") && lines[0].contains("us 10"),
            "{lines:?}"
        );
        // the weak splits are not shown, except the strongest one always is
        assert_eq!(lines.len(), 1, "{lines:?}");
        let r = render(&t, &ctx(), 2500);
        assert!(
            r.text.contains("amount by region (mean): ap "),
            "{}",
            r.text
        );
    }

    #[test]
    fn a_split_with_tiny_groups_or_one_group_is_not_a_breakdown() {
        let mut s = String::from("order,kind,amount\n");
        for i in 0..100usize {
            // 'rare' has 2 rows: too few for a mean
            let kind = if i < 2 { "rare" } else { "common" };
            s.push_str(&format!("{},{kind},{}\n", 1000 + i, (i * 13) % 50));
        }
        let t = detect(&s).unwrap();
        assert!(breakdown_notes(&t, &analyze(&t)).is_empty());
    }

    #[test]
    fn columns_that_say_nothing_get_a_name_and_not_a_line() {
        let mut s = String::from("id,kind,kind_code,actor,actor_id,url,version,note,amount\n");
        for i in 0..80usize {
            let (kind, code) = [("push", 1), ("pull", 2), ("tag", 3)][i % 3];
            let (actor, actor_id) = [("ann", 101), ("bo", 202)][i % 2];
            s.push_str(&format!(
                "{},{kind},{code},{actor},{actor_id},https://x.test/items/{i},v1,{},{}\n",
                7000 + i,
                if i % 10 == 0 { "" } else { "same" },
                (i * 37) % 91
            ));
        }
        let t = detect(&s).unwrap();
        let r = render(&t, &ctx(), 2500);
        // kind and kind_code split the rows the same way; so do actor and actor_id
        assert!(
            r.text.contains("same groups as kind: kind_code"),
            "{}",
            r.text
        );
        assert!(
            r.text.contains("same groups as actor: actor_id"),
            "{}",
            r.text
        );
        assert!(!r.text.contains("\nactor_id: "), "{}", r.text);
        // version is v1 in every row, url is different in every row
        assert!(
            r.text.contains("same in every row: version=v1"),
            "{}",
            r.text
        );
        assert!(
            r.text.contains("different in every row (ids, urls): url"),
            "{}",
            r.text
        );
        assert!(
            r.text.contains("url like https://x.test/items/0"),
            "{}",
            r.text
        );
        assert!(
            !r.text.contains("\nurl: ") && !r.text.contains("\nversion: "),
            "{}",
            r.text
        );
        // the varying columns keep their lines; a column with gaps is not constant
        assert!(r.text.contains("\nkind: 3 distinct: "), "{}", r.text);
        assert!(r.text.contains("\namount: 0.."), "{}", r.text);
        assert!(r.text.contains("\nnote: "), "{}", r.text);
        // the sample rows leave out what the lines above already said
        assert!(r.text.contains("head "), "{}", r.text);
        let rows_header = r.text.lines().find(|l| l.starts_with("head ")).unwrap();
        assert!(
            rows_header.contains("columns: ") && !rows_header.contains("url"),
            "{rows_header}"
        );
    }

    #[test]
    fn hex_ids_are_named_by_kind_not_shown_twice() {
        let mut s = String::from("id,sha,branch\n");
        for i in 0..60usize {
            s.push_str(&format!(
                "{i},{:040x},{}\n",
                (i % 12) as u128 * 0x1234_5678_9abc_def0_1234_5678,
                ["main", "dev", "x"][i % 3]
            ));
        }
        let t = detect(&s).unwrap();
        let c = &analyze(&t)[1];
        assert!(
            c.summary.contains("12 distinct, ") && c.summary.contains("-character hex ids"),
            "{}",
            c.summary
        );
    }

    #[test]
    fn long_value_lists_stop_and_say_how_many_are_left() {
        let mut s = String::from("id,title\n");
        for i in 0..120usize {
            s.push_str(&format!(
                "{i},A rather long title of a pull request number {}\n",
                i % 12
            ));
        }
        let t = detect(&s).unwrap();
        let line = &analyze(&t)[1].summary;
        assert!(line.starts_with("12 distinct: "), "{line}");
        assert!(line.contains(", +") && line.contains(" more"), "{line}");
    }

    #[test]
    fn outliers_are_named_but_a_heavy_tail_is_not() {
        let mut s = String::from("id,amount,wait\n");
        for i in 0..200 {
            let amount = if i == 17 || i == 120 {
                9000.0
            } else {
                100.0 + (i % 21) as f64
            };
            // an exponential-looking tail: many large values
            let wait = 1.0 + ((i % 50) as f64).powi(2);
            s.push_str(&format!("{i},{amount},{wait}\n"));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert!(
            info[1].summary.contains("outliers beyond 3xIQR: 2 above"),
            "{}",
            info[1].summary
        );
        assert!(
            info[1].summary.contains("most extreme 9000"),
            "{}",
            info[1].summary
        );
        assert!(!info[2].summary.contains("outliers"), "{}", info[2].summary);
    }

    #[test]
    fn identifier_columns_get_no_largest_and_smallest() {
        let mut s = String::from("row,user_id,seq,score\n");
        for i in 0..60 {
            s.push_str(&format!(
                "{},{},{},{}\n",
                i % 7,
                (i * 37) % 59 + 1000,
                i * 3,
                (i * 13) % 97
            ));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert!(
            !info[1].summary.contains("max at"),
            "user_id is named like an id: {}",
            info[1].summary
        );
        assert!(
            !info[2].summary.contains("max at"),
            "seq rises at every row: {}",
            info[2].summary
        );
        assert!(info[3].summary.contains("max at"), "{}", info[3].summary);
    }

    #[test]
    fn repeated_rows_are_counted() {
        let mut s = String::from("id,v\n");
        for i in 0..80 {
            s.push_str(&format!("{i},{}\n", i % 9));
        }
        s.push_str("5,5\n5,5\n12,3\n");
        let t = detect(&s).unwrap();
        let note = duplicate_note(&t).unwrap();
        assert_eq!(
            note,
            "duplicates: 3 rows repeat an earlier row (first: row 80 = row 5)"
        );
        let t = detect(&s.replace("5,5\n5,5\n12,3\n", "")).unwrap();
        assert!(duplicate_note(&t).is_none());
    }

    #[test]
    fn counter_columns_are_written_as_a_range() {
        let h: Vec<String> = [
            "id", "feat_0", "feat_1", "feat_2", "feat_3", "x", "y1", "y2", "z",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let rows = vec![
            (0..9)
                .map(|i| if i == 5 || i == 8 {
                    "a".into()
                } else {
                    i.to_string()
                })
                .collect::<Vec<String>>();
            3
        ];
        let t = Table {
            format: crate::table::Format::Csv,
            headers: h.clone(),
            rows,
            notes: vec![],
        };
        let info = analyze(&t);
        assert_eq!(
            compact_cols(&h, &info),
            "id:int feat_0..feat_3:int x:str y1:int y2:int z:str"
        );
    }

    #[test]
    fn messages_that_really_differ_are_not_forced_into_templates() {
        let mut s = String::from("id,note\n");
        for i in 0..100 {
            s.push_str(&format!(
                "{},{}\n",
                i,
                ["alpha beta", "gamma delta", "epsilon", "zeta eta theta"][i % 4].to_string()
                    + &"x".repeat(i % 17)
            ));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert!(
            info[1].stats.templates.is_empty() || !info[1].summary.contains("templates"),
            "{}",
            info[1].summary
        );
    }

    #[test]
    fn numbers_get_percentiles() {
        let mut s = String::from("name,ms\n");
        for i in 0..200 {
            s.push_str(&format!("row{},{}\n", i, i));
        }
        let t = detect(&s).unwrap();
        let info = analyze(&t);
        assert!(
            info[1].summary.contains("p50 100 p95 189"),
            "{}",
            info[1].summary
        );
    }

    #[test]
    fn wide_table_shrinks_to_budget() {
        let mut s = (0..60)
            .map(|i| format!("column_name_{i}"))
            .collect::<Vec<_>>()
            .join(",");
        s.push('\n');
        for r in 0..200 {
            s.push_str(
                &(0..60)
                    .map(|c| format!("value_{}_{}", r, c))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            s.push('\n');
        }
        let t = detect(&s).unwrap();
        let r = render(&t, &ctx(), 2500);
        // each column is a different token in every row, so none gets a statistics line
        assert!(
            r.text
                .contains("different in every row (ids, urls): column_name_0..column_name_59"),
            "{}",
            r.text
        );
        assert!(
            r.text
                .contains("head 3 (columns: column_name_0..column_name_29)"),
            "{}",
            r.text
        );
        // every column is still named, the counter columns as one range
        assert!(
            r.text.contains("column_name_0..column_name_59:str"),
            "{}",
            r.text
        );
    }
}
