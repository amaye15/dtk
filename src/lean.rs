//! Lean views: a small, typed summary of a table that keeps what an agent acts on.

use crate::table::Table;
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
    /// Most common values with their counts, most common first (at most 8).
    pub top: Vec<(String, usize)>,
}

pub struct ColInfo {
    pub kind: Kind,
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

fn top_pairs(distinct: &HashMap<&str, usize>, limit: usize) -> Vec<(String, usize)> {
    let mut v: Vec<(&&str, &usize)> = distinct.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.into_iter()
        .take(limit)
        .map(|(k, n)| (k.to_string(), *n))
        .collect()
}

fn analyze_col<'a>(vals: impl Iterator<Item = &'a str>) -> ColInfo {
    let mut nonempty = 0usize;
    let mut nulls = 0usize;
    let (mut all_int, mut all_float, mut all_bool, mut all_time) = (true, true, true, true);
    let (mut imin, mut imax) = (i64::MAX, i64::MIN);
    let (mut fmin, mut fmax, mut fsum) = (f64::INFINITY, f64::NEG_INFINITY, 0f64);
    let (mut tmin, mut tmax): (Option<&str>, Option<&str>) = (None, None);
    let mut distinct: HashMap<&str, usize> = HashMap::new();
    let mut overflow = false;
    let mut nums: Vec<f64> = Vec::new();
    for v in vals {
        if v.is_empty() {
            nulls += 1;
            continue;
        }
        nonempty += 1;
        let zero = has_leading_zero(v);
        if all_int {
            match v.parse::<i64>() {
                Ok(i) if !zero => {
                    imin = imin.min(i);
                    imax = imax.max(i);
                }
                _ => all_int = false,
            }
        }
        if all_float {
            match v.parse::<f64>() {
                Ok(f) if f.is_finite() && !zero => {
                    fmin = fmin.min(f);
                    fmax = fmax.max(f);
                    fsum += f;
                    nums.push(f);
                }
                _ => {
                    all_float = false;
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
    let pcts = if matches!(kind, Kind::Int | Kind::Float) {
        pct(&mut nums)
    } else {
        None
    };
    let mut summary = match kind {
        Kind::Empty => "all empty".to_string(),
        Kind::Int | Kind::Float if few_values => {
            format!("{} distinct: {}", distinct.len(), top_values(&distinct, 8))
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
                ">5000 distinct".to_string()
            } else if distinct.len() <= 12 {
                format!("{} distinct: {}", distinct.len(), top_values(&distinct, 8))
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
    if nulls > 0 {
        summary.push_str(&format!(" | nulls {}", nulls));
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
        top: top_pairs(&distinct, 8),
    };
    ColInfo {
        kind,
        summary,
        distinct: if overflow { None } else { Some(distinct.len()) },
        stats,
    }
}

pub fn analyze(t: &Table) -> Vec<ColInfo> {
    (0..t.headers.len())
        .map(|c| analyze_col(t.rows.iter().map(|r| r[c].as_str())))
        .collect()
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

fn shell_quote(p: &str) -> String {
    if p.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        p.to_string()
    } else {
        format!("'{}'", p.replace('\'', "'\\''"))
    }
}

const LEVEL_NAMES: [&str; 5] = ["level", "severity", "lvl", "loglevel", "log_level"];
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
        stat_cols: 40,
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

fn summary(t: &Table, info: &[ColInfo], ctx: &Ctx, p: &Profile) -> String {
    let n = t.rows.len();
    let cols = t.headers.len();
    let mut out = String::new();
    out.push_str(&format!(
        "[dtk] {}: {} rows x {} cols, {}. This is a summary of all rows; the full data is on disk.\n",
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
    // A wide table shows its first columns only; the rest is in the data file.
    let k = p.stat_cols.min(cols);
    out.push_str("cols: ");
    out.push_str(
        &t.headers
            .iter()
            .zip(info)
            .take(k)
            .map(|(h, i)| format!("{}:{}", h, i.kind.name()))
            .collect::<Vec<_>>()
            .join(" "),
    );
    if cols > k {
        out.push_str(&format!(" (+{} more columns in the data file)", cols - k));
    }
    out.push('\n');
    for (h, i) in t.headers.iter().zip(info).take(p.stat_cols) {
        out.push_str(&format!("{}: {}\n", h, i.summary));
    }
    if cols > p.stat_cols {
        out.push_str(&format!(
            "... {} more columns not summarized\n",
            cols - p.stat_cols
        ));
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
        if !bad.is_empty() {
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

fn drill(t: &Table, info: &[ColInfo], ctx: &Ctx) -> String {
    let mut s = String::new();
    let col = info
        .iter()
        .position(|i| i.kind == Kind::Str && i.distinct.is_some_and(|d| d > 1 && d <= 50))
        .unwrap_or(0);
    let name = t.headers[col].replace('"', "");
    if let Some(p) = &ctx.csv_path {
        let p = shell_quote(p);
        s.push_str(&format!(
            "drill: qsv sqlp {p} 'select \"{name}\", count(*) n from _t_1 group by \"{name}\" order by n desc'\n"
        ));
        s.push_str(&format!(
            "rows: qsv slice -s <start> -l <count> {p}   (start counts from 0)\n"
        ));
    }
    if let Some(p) = &ctx.raw_path {
        if matches!(
            t.format,
            crate::table::Format::Jsonl | crate::table::Format::JsonArray
        ) {
            let q = shell_quote(p);
            let pick = format!("select(.[\"{name}\"]==\"<value>\")");
            let filter = if t.format == crate::table::Format::Jsonl {
                pick
            } else {
                format!(".[] | {pick}")
            };
            s.push_str(&format!("json: jaq -c '{filter}' {q}\n"));
        }
    }
    s
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
        assert!(r.text.contains("qsv sqlp /tmp/x.csv"));
        assert!(est_tokens(&r.text) <= 2500);
    }

    #[test]
    fn warn_and_error_rows_are_kept() {
        let t = detect(&fx::jsonl_text(400)).unwrap();
        let r = render(&t, &ctx(), 2500);
        assert!(r
            .text
            .contains("rows with level=warn/error: showing 20 of 200"));
        assert!(r
            .text
            .contains("jaq -c 'select(.[\"level\"]==\"<value>\")'"));
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
        assert!(r.text.contains("more columns not summarized"));
    }
}
