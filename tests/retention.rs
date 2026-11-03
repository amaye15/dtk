//! Gate G6: what a summary keeps.
//!
//! A shorter output only helps if the model can still answer the questions it would have
//! answered from the full output. For every table in the corpus that dtk summarizes, this test
//! lists the facts those questions need, computes them from the parsed rows with its own
//! arithmetic (not with dtk's), and checks that the summary text states each one:
//!
//! | question | fact in the summary |
//! |---|---|
//! | how many rows? | the row count |
//! | what columns? | every column name |
//! | what is the range of X? | the smallest and the largest value of each numeric column |
//! | which row has the largest (or smallest) X? | the row's id (or its number) beside the value |
//! | how many rows have X = v? | each value of a column with at most 12 values, with its count |
//! | what period does it cover? | the first and the last value of each time column |
//! | what is the first (last) row? | the head and tail rows |
//!
//! A fact that is missing is a question the model must spend another tool call on, or guess.
//! The test prints the score per fixture; a fact missing from a fixture fails the test.

use dtk::lean::{analyze, render, Ctx, Kind};
use dtk::table::{detect, Table};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const BUDGET: usize = 2500;

fn corpus() -> (PathBuf, Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let m = serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    (dir, m)
}

const PLACEHOLDERS: [&str; 13] = [
    "n/a", "na", "nan", "null", "none", "nil", "-", "--", "?", "#n/a", "<na>", "(null)", "missing",
];

/// A number as people write it: `$1,234.56`, `45.3%`, `1,234`. Simple on purpose, and not dtk's code.
fn loose(s: &str) -> Option<f64> {
    let t = s
        .trim()
        .trim_start_matches(['$', '€', '£', '¥'])
        .trim_end_matches('%')
        .replace(',', "");
    if t.is_empty() || (t.len() > 1 && t.starts_with('0') && !t.starts_with("0.")) {
        return None;
    }
    t.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn num(s: &str) -> Option<f64> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0') && !s.starts_with("0.")) {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// The number as the summary may print it: the cell as written, or Rust's shortest form.
fn spellings(cell: &str, v: f64) -> [String; 2] {
    [cell.to_string(), format!("{v}")]
}

/// The column that names a row: the first of the first three whose values are all different, not
/// empty and at most 40 characters (an order id, a timestamp); else rows are named by position.
fn id_column(t: &Table) -> Option<usize> {
    (0..t.headers.len().min(3)).find(|&c| {
        let mut seen = std::collections::HashSet::new();
        t.rows
            .iter()
            .all(|r| !r[c].is_empty() && r[c].chars().count() <= 40 && seen.insert(r[c].as_str()))
    })
}

struct Fact {
    what: String,
    /// Text that must appear, or any of several spellings.
    any_of: Vec<String>,
    /// Only the summary line of this column is searched, when set.
    in_line_of: Option<String>,
    /// When not empty: some row line (not a line of statistics) must hold all of these.
    same_line: Vec<String>,
}

fn facts(t: &Table) -> Vec<Fact> {
    let kinds = analyze(t);
    let n = t.rows.len();
    let mut out = Vec::new();
    let mut fact = |what: String, any_of: Vec<String>, line: Option<&str>| {
        out.push(Fact {
            what,
            any_of,
            in_line_of: line.map(String::from),
            same_line: Vec::new(),
        })
    };
    // The full rows of the extremes of the first measure: (what, strings one row line must hold).
    let mut full_rows: Vec<(String, Vec<String>)> = Vec::new();
    fact("row count".into(), vec![format!("{n} rows")], None);
    for h in &t.headers {
        fact(format!("column name {h}"), vec![h.clone()], None);
    }
    let id = id_column(t);
    let mut outlier_facts = 0;
    // dtk names the row of the extremes for the first five columns only (a token limit).
    let mut which_row_cols = 0;
    // Repeated rows are counted.
    let mut seen = std::collections::HashSet::new();
    let repeats = t.rows.iter().filter(|r| !seen.insert(*r)).count();
    if repeats > 0 {
        fact(
            format!("{repeats} repeated rows are counted"),
            vec![format!("duplicates: {repeats} row")],
            None,
        );
    }
    for (c, h) in t.headers.iter().enumerate() {
        let col: Vec<&str> = t.rows.iter().map(|r| r[c].as_str()).collect();
        let nonempty: Vec<&str> = col.iter().copied().filter(|v| !v.is_empty()).collect();
        if nonempty.is_empty() {
            continue;
        }
        let nums: Vec<Option<f64>> = nonempty.iter().map(|v| num(v)).collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for v in &nonempty {
            *counts.entry(v).or_default() += 1;
        }
        // Codes (HTTP status, a restart count) are asked about by value, not by range.
        let codes = counts.len() <= 12 && nonempty.len() >= 2 * counts.len();
        if nums.iter().all(Option::is_some)
            && matches!(kinds[c].kind, Kind::Int | Kind::Float)
            && !codes
        {
            let vals: Vec<f64> = nums.iter().map(|x| x.unwrap()).collect();
            let (lo, hi) = (
                vals.iter().cloned().fold(f64::INFINITY, f64::min),
                vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            );
            let cell_of = |v: f64| nonempty[vals.iter().position(|x| *x == v).unwrap()];
            fact(
                format!("{h}: smallest value {lo}"),
                spellings(cell_of(lo), lo).to_vec(),
                Some(h),
            );
            fact(
                format!("{h}: largest value {hi}"),
                spellings(cell_of(hi), hi).to_vec(),
                Some(h),
            );
            // Which row holds the extremes? Not asked of a column that is itself the id, of a
            // constant column, or of a column that is the row's position in disguise.
            let unique =
                vals.windows(2).all(|w| w[0] < w[1]) || vals.windows(2).all(|w| w[0] > w[1]);
            if Some(c) != id
                && hi > lo
                && !unique
                && !(id_name(h) && kinds[c].kind == Kind::Int)
                && which_row_cols < 5
            {
                which_row_cols += 1;
                if which_row_cols == 1 {
                    let r = (0..n).find(|&r| num(&t.rows[r][c]) == Some(hi)).unwrap();
                    let label_col = id.unwrap_or(0);
                    full_rows.push((
                        format!("{h}: the full row of the largest value is shown"),
                        vec![t.rows[r][label_col].clone(), t.rows[r][c].clone()],
                    ));
                }
                for (label, target) in [("largest", hi), ("smallest", lo)] {
                    let r = (0..n)
                        .find(|&r| num(&t.rows[r][c]) == Some(target))
                        .unwrap();
                    let who = match id {
                        Some(ic) => format!("{}={}", t.headers[ic], t.rows[r][ic]),
                        None => format!("row {r}"),
                    };
                    fact(
                        format!("{h}: which row has the {label} value ({target}) -> {who}"),
                        vec![who],
                        Some(h),
                    );
                }
            }
        }
        if kinds[c].kind == Kind::Time {
            // ISO times compare as text. Dates like 03/21/2025 are put in order here by hand: the
            // data says which part is the month when a second part is above 12.
            let iso = nonempty.iter().all(|v| v.as_bytes().get(4) == Some(&b'-'));
            let (lo, hi) = if iso {
                (
                    nonempty.iter().min().unwrap().to_string(),
                    nonempty.iter().max().unwrap().to_string(),
                )
            } else {
                let parts: Vec<(u32, u32, u32)> = nonempty
                    .iter()
                    .filter(|v| !PLACEHOLDERS.contains(&v.to_lowercase().as_str()))
                    .map(|v| {
                        let p: Vec<u32> = v
                            .split(['/', '-', '.'])
                            .map(|x| x.parse().unwrap())
                            .collect();
                        (p[0], p[1], p[2])
                    })
                    .collect();
                let month_first = parts.iter().any(|p| p.1 > 12);
                let key = |p: &(u32, u32, u32)| {
                    let (m, d) = if month_first { (p.0, p.1) } else { (p.1, p.0) };
                    (p.2, m, d)
                };
                let (lo, hi) = (
                    parts.iter().map(key).min().unwrap(),
                    parts.iter().map(key).max().unwrap(),
                );
                (
                    format!("{:04}-{:02}-{:02}", lo.0, lo.1, lo.2),
                    format!("{:04}-{:02}-{:02}", hi.0, hi.1, hi.2),
                )
            };
            fact(format!("{h}: first time {lo}"), vec![lo], Some(h));
            fact(format!("{h}: last time {hi}"), vec![hi], Some(h));
            // Are the rows in time order? ISO times compare as text.
            if iso && nonempty.len() >= 20 && nonempty.len() * 10 >= n * 9 {
                let up = nonempty.windows(2).all(|w| w[0] <= w[1]);
                let down = nonempty.windows(2).all(|w| w[0] >= w[1]);
                let want = match (up, down) {
                    (true, true) => None,
                    (true, false) => Some("sorted oldest first"),
                    (false, true) => Some("sorted newest first"),
                    _ => Some("not in time order"),
                };
                if let Some(w) = want {
                    fact(format!("{h}: {w}"), vec![w.to_string()], Some(h));
                }
            }
        }
        // Real-world columns: numbers written as `$1,234.56` or `45%`, `N/A` and `-` for missing.
        // The numbers keep their range, and the missing cells are counted, not mistaken for words.
        let real: Vec<&str> = nonempty
            .iter()
            .copied()
            .filter(|v| !PLACEHOLDERS.contains(&v.to_lowercase().as_str()))
            .collect();
        let placeholders = nonempty.len() - real.len();
        let loose_vals: Vec<f64> = real.iter().filter_map(|v| loose(v)).collect();
        let all_loose =
            !real.is_empty() && loose_vals.len() == real.len() && placeholders < real.len();
        let strict = nums.iter().all(Option::is_some);
        if all_loose && matches!(kinds[c].kind, Kind::Int | Kind::Float) && !codes {
            let (lo, hi) = (
                loose_vals.iter().cloned().fold(f64::INFINITY, f64::min),
                loose_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            );
            if !strict {
                fact(
                    format!("{h}: smallest value {lo}"),
                    vec![format!("{lo}")],
                    Some(h),
                );
                fact(
                    format!("{h}: largest value {hi}"),
                    vec![format!("{hi}")],
                    Some(h),
                );
            }
            if placeholders > 0 {
                fact(
                    format!("{h}: {placeholders} placeholders are counted"),
                    vec![format!("{placeholders} placeholders excluded")],
                    Some(h),
                );
            }
            // Outliers: a few values beyond three interquartile ranges from the quartiles.
            let rises = loose_vals.windows(2).all(|w| w[0] < w[1])
                || loose_vals.windows(2).all(|w| w[0] > w[1]);
            let identifier = (id_name(h) && kinds[c].kind == Kind::Int) || rises;
            if loose_vals.len() >= 20 && hi > lo && !identifier && outlier_facts < 5 {
                let mut sorted = loose_vals.clone();
                sorted.sort_by(|a, b| a.total_cmp(b));
                let m = sorted.len();
                let (q1, q3) = (sorted[(m - 1) / 4], sorted[3 * (m - 1) / 4]);
                let iqr = q3 - q1;
                let (above, below): (Vec<f64>, Vec<f64>) = (
                    sorted
                        .iter()
                        .copied()
                        .filter(|v| *v > q3 + 3.0 * iqr)
                        .collect(),
                    sorted
                        .iter()
                        .copied()
                        .filter(|v| *v < q1 - 3.0 * iqr)
                        .collect(),
                );
                let total = above.len() + below.len();
                if iqr > 0.0 && total > 0 && total <= (m / 20).max(3) {
                    outlier_facts += 1;
                    if let Some(x) = above.last() {
                        fact(
                            format!("{h}: outlier {x}"),
                            vec![format!("most extreme {x}")],
                            Some(h),
                        );
                    }
                    if let Some(x) = below.first() {
                        fact(
                            format!("{h}: outlier {x}"),
                            vec![format!("most extreme {x}")],
                            Some(h),
                        );
                    }
                }
            }
        }
        // Values that count: a column of at most 12 values that are words, not numbers.
        let words = nums.iter().any(Option::is_none) && kinds[c].kind != Kind::Time;
        let numeric_codes = !words && codes && matches!(kinds[c].kind, Kind::Int | Kind::Float);
        if counts.len() <= 12 && counts.len() >= 2 && (words || numeric_codes) {
            // A value of 40 characters or more is a blob (nested JSON), not something to count.
            for (v, k) in counts.iter().filter(|(v, _)| v.chars().count() < 25) {
                fact(
                    format!("{h}: {v} occurs {k} times"),
                    vec![format!("{v} {k}")],
                    Some(h),
                );
            }
        }
    }
    // Columns that move together: |Pearson r| of 0.9 or more among the first twelve measures.
    let mut measures: Vec<(usize, Vec<Option<f64>>)> = Vec::new();
    for (c, h) in t.headers.iter().enumerate() {
        if measures.len() == 12 {
            break;
        }
        let vals: Vec<Option<f64>> = t.rows.iter().map(|r| loose(&r[c])).collect();
        let present: Vec<f64> = vals.iter().flatten().copied().collect();
        let distinct: std::collections::HashSet<String> =
            t.rows.iter().map(|r| r[c].clone()).collect();
        let codes = distinct.len() <= 12 && present.len() >= 2 * distinct.len();
        let rises = present.len() >= 20
            && distinct.len() == present.len()
            && (present.windows(2).all(|w| w[0] < w[1]) || present.windows(2).all(|w| w[0] > w[1]));
        let numeric = matches!(kinds[c].kind, Kind::Int | Kind::Float);
        if !numeric || codes || Some(c) == id || rises || (id_name(h) && kinds[c].kind == Kind::Int)
        {
            continue;
        }
        measures.push((c, vals));
    }
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, (ca, a)) in measures.iter().enumerate() {
        for (cb, b) in &measures[i + 1..] {
            let both: Vec<(f64, f64)> = a
                .iter()
                .zip(b)
                .filter_map(|(x, y)| Some(((*x)?, (*y)?)))
                .collect();
            let m = both.len() as f64;
            if m < 20.0 {
                continue;
            }
            let (ma, mb) = (
                both.iter().map(|p| p.0).sum::<f64>() / m,
                both.iter().map(|p| p.1).sum::<f64>() / m,
            );
            let (cov, va, vb) = both.iter().fold((0.0, 0.0, 0.0), |s, p| {
                (
                    s.0 + (p.0 - ma) * (p.1 - mb),
                    s.1 + (p.0 - ma).powi(2),
                    s.2 + (p.1 - mb).powi(2),
                )
            });
            if va > 0.0 && vb > 0.0 && (cov / (va * vb).sqrt()).abs() >= 0.9 {
                pairs.push((cov / (va * vb).sqrt(), *ca, *cb));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
    let cutoff = pairs.get(3).map_or(0.0, |p| p.0.abs());
    for (r, a, b) in pairs.iter().take(3).filter(|p| p.0.abs() > cutoff + 1e-9) {
        let (ha, hb) = (&t.headers[*a], &t.headers[*b]);
        fact(
            format!("{ha} and {hb} move together (r = {r:.2})"),
            vec![format!("{ha}~{hb}")],
            None,
        );
    }
    // Group means for the pair people ask about first: the leftmost column with a few values
    // against the leftmost measure. Every group of five or more rows has its mean in the summary.
    if t.headers.len() <= 40 {
        let sig4 = |x: f64| -> String {
            if x == 0.0 {
                return "0".into();
            }
            let d = (3 - x.abs().log10().floor() as i32).clamp(0, 8) as usize;
            let s = format!("{x:.d$}");
            if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                s
            }
        };
        let measure_cols: Vec<usize> = (0..t.headers.len())
            .filter(|&c| {
                let vals: Vec<f64> = t.rows.iter().filter_map(|r| loose(&r[c])).collect();
                let distinct: std::collections::HashSet<&str> =
                    t.rows.iter().map(|r| r[c].as_str()).collect();
                let codes = distinct.len() <= 12 && vals.len() >= 2 * distinct.len();
                let rises = vals.len() >= 20
                    && distinct.len() == vals.len()
                    && (vals.windows(2).all(|w| w[0] < w[1])
                        || vals.windows(2).all(|w| w[0] > w[1]));
                matches!(kinds[c].kind, Kind::Int | Kind::Float)
                    && !codes
                    && Some(c) != id
                    && !rises
                    && !(id_name(&t.headers[c]) && kinds[c].kind == Kind::Int)
            })
            .take(6)
            .collect();
        type Primary = (usize, usize, Vec<(String, f64)>);
        let mut primary: Option<Primary> = None;
        'search: for c in 0..t.headers.len() {
            let values: Vec<&str> = t
                .rows
                .iter()
                .map(|r| r[c].as_str())
                .filter(|v| !v.is_empty() && !PLACEHOLDERS.contains(&v.to_lowercase().as_str()))
                .collect();
            let distinct: std::collections::HashSet<&str> = values.iter().copied().collect();
            let codes = distinct.len() <= 12 && values.len() >= 2 * distinct.len();
            let labelled = matches!(kinds[c].kind, Kind::Str | Kind::Bool)
                || (codes && kinds[c].kind == Kind::Int);
            let is_measure = measure_cols.contains(&c) && !(codes && kinds[c].kind == Kind::Int);
            if !(2..=8).contains(&distinct.len()) || !labelled || Some(c) == id || is_measure {
                continue;
            }
            for &m in &measure_cols {
                if m == c {
                    continue;
                }
                let mut groups: std::collections::BTreeMap<&str, Vec<f64>> =
                    std::collections::BTreeMap::new();
                let mut all = Vec::new();
                for r in &t.rows {
                    let g = r[c].as_str();
                    if g.is_empty() || PLACEHOLDERS.contains(&g.to_lowercase().as_str()) {
                        continue;
                    }
                    if let Some(x) = loose(&r[m]) {
                        groups.entry(g).or_default().push(x);
                        all.push(x);
                    }
                }
                let mean = all.iter().sum::<f64>() / all.len().max(1) as f64;
                let ss_total: f64 = all.iter().map(|x| (x - mean).powi(2)).sum();
                let kept: Vec<(String, f64)> = groups
                    .iter()
                    .filter(|(_, v)| v.len() >= 5)
                    .map(|(g, v)| (g.to_string(), v.iter().sum::<f64>() / v.len() as f64))
                    .collect();
                if all.len() >= 20 && kept.len() >= 2 && ss_total > 0.0 {
                    primary = Some((c, m, kept));
                    break 'search;
                }
            }
        }
        if let Some((c, m, kept)) = primary {
            let (hc, hm) = (&t.headers[c], &t.headers[m]);
            fact(
                format!("{hm} by {hc}: the line is there"),
                vec![format!("{hm} by {hc} (mean):")],
                None,
            );
            for (g, mean) in kept {
                let label: String = if g.chars().count() > 24 {
                    format!("{}…", g.chars().take(24).collect::<String>())
                } else {
                    g.clone()
                };
                fact(
                    format!("{hm} by {hc}: mean for {g} is {}", sig4(mean)),
                    vec![format!("{label} {}", sig4(mean))],
                    None,
                );
            }
        }
    }
    // Trouble: the first few rows with an error-like level are named in the summary (by their
    // first value, which for a log is the time).
    if let Some(lc) = t.headers.iter().position(|h| {
        ["level", "severity", "lvl", "loglevel", "log_level", "flag"]
            .contains(&h.to_lowercase().as_str())
    }) {
        let bad: Vec<usize> = (0..n)
            .filter(|&r| {
                ["error", "err", "fatal", "critical", "crit", "panic"]
                    .contains(&t.rows[r][lc].to_lowercase().as_str())
            })
            .take(5)
            .collect();
        for r in bad {
            fact(
                format!("error row {r} ({}) is shown", t.rows[r][0]),
                vec![t.rows[r][0].clone()],
                None,
            );
        }
    }
    let first = t.rows[0][0].clone();
    let last = t.rows[n - 1][0].clone();
    fact("first row".into(), vec![first], None);
    fact("last row".into(), vec![last], None);
    for (what, same_line) in full_rows {
        out.push(Fact {
            what,
            any_of: Vec::new(),
            in_line_of: None,
            same_line,
        });
    }
    out
}

/// A column that numbers or names rows (`order_id`, `uuid`, `index`): the largest one is no news.
fn id_name(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "id", "uuid", "guid", "index", "idx", "key", "pk", "rowid", "#", "no", "num",
    ]
    .contains(&n.as_str())
        || n.ends_with("_id")
        || n.ends_with("-id")
        || n.ends_with("id") && n.len() > 2 && !n.ends_with("paid") && !n.ends_with("valid")
}

/// A column is named when its name is printed, or when it falls inside a printed range such as
/// `feat_0..feat_119:float`.
fn named_in(text: &str, name: &str) -> bool {
    if text.contains(name) {
        return true;
    }
    let split = |s: &str| -> Option<(String, u64)> {
        let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (p, n) = s.split_at(s.len() - digits);
        Some((p.to_string(), n.parse().ok()?))
    };
    let Some((prefix, num)) = split(name) else {
        return false;
    };
    text.split_whitespace().any(|tok| {
        let tok = tok.trim_matches(|c| "(),;:".contains(c));
        let Some((a, b)) = tok.split_once("..") else {
            return false;
        };
        let b = b.rsplit_once(':').map_or(b, |x| x.0);
        match (split(a), split(b)) {
            (Some((pa, na)), Some((pb, nb))) => {
                pa == prefix && pb == prefix && na <= num && num <= nb
            }
            _ => false,
        }
    })
}

/// Why a column has no line of statistics, worked out here from its cells, and whether the summary
/// says so. A claim is only accepted when it is true.
fn accounted(text: &str, t: &Table, c: usize) -> Result<(), String> {
    let n = t.rows.len();
    let h = &t.headers[c];
    let cells: Vec<&str> = t.rows.iter().map(|r| r[c].as_str()).collect();
    let nonempty = cells.iter().filter(|v| !v.is_empty()).count();
    let distinct: std::collections::HashSet<&str> =
        cells.iter().copied().filter(|v| !v.is_empty()).collect();
    let line_starting = |prefix: &str| text.lines().find(|l| l.starts_with(prefix)).unwrap_or("");
    let says = |prefix: &str, needle: &str| line_starting(prefix).contains(needle);
    // A list cut short ends in `+N more`: accepted for a name that is not shown.
    let cut_short = |line: &str| {
        line.rsplit_once(", +").is_some_and(|(_, r)| {
            r.ends_with(" more") && r.starts_with(|c: char| c.is_ascii_digit())
        })
    };
    // 1. the same value in every row (or empty in every row)
    if nonempty == 0 || (distinct.len() == 1 && nonempty >= 2 && nonempty * 2 >= n) {
        let also = text
            .lines()
            .find(|l| l.trim_start().starts_with("also the same in every row:"))
            .unwrap_or("");
        return if says("same in every row:", &format!("{h}="))
            || named_in(also, h)
            || cut_short(also)
        {
            Ok(())
        } else {
            Err("constant, but not in the 'same in every row' line".into())
        };
    }
    // 2. the same groups as an earlier column
    let groups = |col: usize| -> Vec<usize> {
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        t.rows
            .iter()
            .map(|r| {
                let k = seen.len();
                *seen.entry(r[col].as_str()).or_insert(k)
            })
            .collect()
    };
    if cells.iter().all(|v| !v.is_empty()) && (2..n).contains(&distinct.len()) {
        let mine = groups(c);
        for e in 0..c {
            let theirs: std::collections::HashSet<&str> =
                t.rows.iter().map(|r| r[e].as_str()).collect();
            if t.rows.iter().all(|r| !r[e].is_empty())
                && (2..n).contains(&theirs.len())
                && groups(e) == mine
            {
                let prefix = format!("same groups as {}:", t.headers[e]);
                let line = line_starting(&prefix);
                return if named_in(line, h) || cut_short(line) {
                    Ok(())
                } else {
                    Err(format!("a copy of {}, but not said", t.headers[e]))
                };
            }
        }
    }
    // 3. a different token in every row
    let kinds = analyze(t);
    if kinds[c].kind == Kind::Str
        && nonempty >= 10
        && distinct.len() == nonempty
        && nonempty * 10 >= n * 9
        && cells
            .iter()
            .take(200)
            .all(|v| !v.contains(char::is_whitespace))
    {
        let line = line_starting("different in every row");
        return if named_in(line, h) || cut_short(line) {
            Ok(())
        } else {
            Err("unique, but not in the 'different in every row' line".into())
        };
    }
    // 4. left out for lack of room, and said so
    let line = line_starting("no statistics shown for");
    if named_in(line, h) || (cut_short(line) && !line.is_empty()) {
        return Ok(());
    }
    Err("no line, and not accounted for".into())
}

fn line_of<'a>(text: &'a str, header: &str) -> &'a str {
    let key = format!("{header}: ");
    text.lines().find(|l| l.starts_with(&key)).unwrap_or("")
}

#[test]
fn summaries_keep_the_facts_that_questions_need() {
    let (dir, m) = corpus();
    let (mut total, mut kept) = (0usize, 0usize);
    let mut missing = Vec::new();
    for e in m["positive"].as_array().unwrap() {
        let file = e["file"].as_str().unwrap();
        let text = String::from_utf8_lossy(&fs::read(dir.join(file)).unwrap()).to_string();
        let Some(t) = detect(&text) else { continue };
        let ctx = Ctx {
            bytes: text.len(),
            csv_path: None,
            raw_path: None,
        };
        let r = render(&t, &ctx, BUDGET);
        if r.lossless {
            continue; // every row is shown, nothing to keep
        }
        let (mut f_total, mut f_kept) = (0, 0);
        for f in facts(&t) {
            let hay = match &f.in_line_of {
                Some(h) => line_of(&r.text, h),
                None => r.text.as_str(),
            };
            // A column without a statistics line is accounted for below: it is the same in every
            // row, a copy of another column, different in every row, or named as left out.
            if f.in_line_of
                .as_ref()
                .is_some_and(|h| line_of(&r.text, h).is_empty())
            {
                continue;
            }

            f_total += 1;
            let kept = if !f.same_line.is_empty() {
                // a row line: not a line of statistics (`name: ...`)
                r.text.lines().any(|l| {
                    !t.headers.iter().any(|h| l.starts_with(&format!("{h}: ")))
                        && f.same_line.iter().all(|s| l.contains(s.as_str()))
                })
            } else if f.what.starts_with("column name ") {
                named_in(hay, &f.any_of[0])
            } else {
                f.any_of.iter().any(|s| hay.contains(s.as_str()))
            };
            if kept {
                f_kept += 1;
            } else {
                missing.push(format!(
                    "{file}: {}",
                    f.what.chars().take(110).collect::<String>()
                ));
            }
        }
        // Every column without a line of statistics must be accounted for, truthfully.
        for c in 0..t.headers.len() {
            if !line_of(&r.text, &t.headers[c]).is_empty() {
                continue;
            }
            f_total += 1;
            match accounted(&r.text, &t, c) {
                Ok(()) => f_kept += 1,
                Err(why) => missing.push(format!("{file}: column {} - {why}", t.headers[c])),
            }
        }
        println!("{file:46} {f_kept:3}/{f_total:3} facts kept");
        total += f_total;
        kept += f_kept;
    }
    println!("TOTAL {kept}/{total} facts kept");
    assert!(
        missing.is_empty(),
        "{} of {total} facts are missing from the summaries:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}
