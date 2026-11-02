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
        })
    };
    fact("row count".into(), vec![format!("{n} rows")], None);
    for h in &t.headers {
        fact(format!("column name {h}"), vec![h.clone()], None);
    }
    let id = id_column(t);
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
            if Some(c) != id && hi > lo && !unique {
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
            let (lo, hi) = (
                nonempty.iter().min().unwrap().to_string(),
                nonempty.iter().max().unwrap().to_string(),
            );
            fact(format!("{h}: first time {lo}"), vec![lo], Some(h));
            fact(format!("{h}: last time {hi}"), vec![hi], Some(h));
        }
        // Values that count: a column of at most 12 values that are words, not numbers.
        let words = nums.iter().any(Option::is_none) && kinds[c].kind != Kind::Time;
        let numeric_codes = !words && codes && matches!(kinds[c].kind, Kind::Int | Kind::Float);
        if counts.len() <= 12 && counts.len() >= 2 && (words || numeric_codes) {
            // A value of 40 characters or more is a blob (nested JSON), not something to count.
            for (v, k) in counts.iter().filter(|(v, _)| v.chars().count() < 40) {
                fact(
                    format!("{h}: {v} occurs {k} times"),
                    vec![format!("{v} {k}")],
                    Some(h),
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
    out
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
        let wide = r.text.contains(" more columns");
        for f in facts(&t) {
            let hay = match &f.in_line_of {
                Some(h) => line_of(&r.text, h),
                None => r.text.as_str(),
            };
            // A wide table says that it leaves columns out. A fact about one of them is
            // disclosed as absent, not lost. (Every column fact starts with its name.)
            if wide
                && f.in_line_of
                    .as_ref()
                    .is_some_and(|h| line_of(&r.text, h).is_empty())
            {
                continue;
            }
            if wide && f.what.starts_with("column name ") && !r.text.contains(&f.any_of[0]) {
                continue;
            }
            f_total += 1;
            if f.any_of.iter().any(|s| hay.contains(s.as_str())) {
                f_kept += 1;
            } else {
                missing.push(format!(
                    "{file}: {}",
                    f.what.chars().take(110).collect::<String>()
                ));
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
