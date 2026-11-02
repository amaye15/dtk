//! Lean views: a small, typed summary of a table that keeps what an agent acts on.

use crate::table::{Format, Table};
use std::collections::HashMap;

/// Tables up to this many rows are re-encoded as CSV and nothing is dropped.
pub const LOSSLESS_ROWS: usize = 60;
/// ...but only when that CSV is no bigger than this many estimated tokens.
const LOSSLESS_TOKENS: usize = 1500;

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

fn fnum(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let s = format!("{:.4}", x);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn top_values(distinct: &HashMap<&str, usize>, limit: usize) -> String {
    let mut v: Vec<(&&str, &usize)> = distinct.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.iter()
        .take(limit)
        .map(|(k, n)| format!("{} {}", k.chars().take(40).collect::<String>(), n))
        .collect::<Vec<_>>()
        .join(", ")
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
            fnum(fmin),
            fnum(fmax),
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
                format!(
                    "{} distinct{}, e.g. {}",
                    distinct.len(),
                    unique,
                    top_values(&distinct, 2)
                )
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

fn trunc(s: &str, max: usize) -> String {
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
        head: 8,
        tail: 4,
        important: 20,
        max_cell: 60,
        stat_cols: 30,
    },
    Profile {
        head: 4,
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
    let mut last = String::new();
    for p in PROFILES.iter() {
        last = summary(t, &info, ctx, p);
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

fn summary(t: &Table, info: &[ColInfo], ctx: &Ctx, p: &Profile) -> String {
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
    // Every column is named. A wide table gets statistics for its first columns only.
    // A very wide table (dozens of features) gets statistics for a dozen columns; the model asks
    // for the others with `dtk stats`, and every column is named above.
    let k = if cols > VERY_WIDE_COLS {
        p.stat_cols.min(12)
    } else {
        p.stat_cols.min(cols)
    };
    out.push_str(&format!("cols: {}\n", compact_cols(&t.headers, info)));
    for (h, i) in t.headers.iter().zip(info).take(k) {
        out.push_str(&format!("{}: {}\n", h, i.summary));
    }
    if cols > k {
        out.push_str(&format!(
            "no statistics shown for the other {} columns ({}..{}): {} stats {} NAME...\n",
            cols - k,
            t.headers[k],
            t.headers[cols - 1],
            command_name(),
            data_ref(ctx)
        ));
    }
    if let Some(note) = duplicate_note(t) {
        out.push_str(&note);
        out.push('\n');
    }
    let clip = |rows: &[&Vec<String>]| -> Vec<Vec<String>> {
        rows.iter().map(|r| r[..k].to_vec()).collect()
    };
    let show = |rows: &[&Vec<String>]| -> String {
        let clipped = clip(rows);
        let refs: Vec<&Vec<String>> = clipped.iter().collect();
        write_csv(None, &refs, Some(p.max_cell))
    };
    let head: Vec<&Vec<String>> = t.rows.iter().take(p.head).collect();
    out.push_str(&format!(
        "head {} (columns in order above):\n{}\n",
        head.len(),
        show(&head)
    ));
    let tail_from = n.saturating_sub(p.tail).max(p.head.min(n));
    let tail: Vec<&Vec<String>> = t.rows.iter().skip(tail_from).collect();
    out.push_str(&format!("tail {}:\n{}\n", tail.len(), show(&tail)));
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
                    shape_lines
                        .push_str(&format!("  {n}x {shape}   (first at {})\n", t.rows[*at][0]));
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
            let shown: Vec<&Vec<String>> = keep.iter().map(|&i| &t.rows[i]).collect();
            out.push_str(&format!(
                "{} error{}, {} warning{} ({}).\n{}showing {} lines, each {}error with the {} lines before it:\n{}\n",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" },
                warns.len(),
                if warns.len() == 1 { "" } else { "s" },
                t.headers[li],
                shape_lines,
                shown.len(),
                if errors.len() > 5 && msg_col.is_some() { "first " } else { "" },
                CONTEXT_LINES,
                show(&shown)
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
    let mut cmds: Vec<String> = Vec::new();
    if let Some(l) = label {
        cmds.push(format!("{dtk} count {f} {}", word(&t.headers[l])));
    }
    if let Some(m) = measure {
        cmds.push(format!("{dtk} top {f} {} 10", word(&t.headers[m])));
    }
    if let Some((l, top)) = label.and_then(|l| Some((l, info[l].stats.top.first()?))) {
        cmds.push(format!(
            "{dtk} where {f} {} = {}",
            word(&t.headers[l]),
            word(&top.0)
        ));
    }
    cmds.push(format!("{dtk} rows {f} 100 20"));
    cmds.push(format!("{dtk} stats {f} NAME..."));
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
            r.text.contains("drill: dtk count /tmp/x.csv name  |  dtk top /tmp/x.csv amount 10  |  dtk where /tmp/x.csv name = user_0"),
            "{}",
            r.text
        );
        assert!(
            r.text
                .contains("dtk rows /tmp/x.csv 100 20  |  dtk stats /tmp/x.csv NAME..."),
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
        assert!(
            r.text.contains("no statistics shown for the other"),
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
