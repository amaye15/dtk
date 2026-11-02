//! Gate G3: properties that must hold for any input.
//!
//! - detection never panics, whatever the bytes;
//! - a detected table is always well formed (equal-width rows, unique non-empty headers);
//! - rendering never panics, and a rendered view is never much bigger than its input;
//! - for every real fixture big enough to be summarized, the summary is much smaller.
//!
//! The random generator is a fixed-seed xorshift, so a failure is reproducible.

use dtk::lean::{est_tokens, render, Ctx};
use dtk::table::{detect, Table};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn well_formed(t: &Table) -> Result<(), String> {
    if t.headers.len() < 2 {
        return Err("fewer than two columns".into());
    }
    let unique: HashSet<&String> = t.headers.iter().collect();
    if unique.len() != t.headers.len() || t.headers.iter().any(|h| h.trim().is_empty()) {
        return Err(format!("bad headers {:?}", t.headers));
    }
    match t.rows.iter().position(|r| r.len() != t.headers.len()) {
        Some(i) => Err(format!(
            "row {i} has {} cells, expected {}",
            t.rows[i].len(),
            t.headers.len()
        )),
        None => Ok(()),
    }
}

fn ctx(text: &str) -> Ctx {
    Ctx {
        bytes: text.len(),
        csv_path: Some("/tmp/x.csv".into()),
        raw_path: Some("/tmp/x.raw".into()),
    }
}

/// Detect, check, render, check. Returns the table when there was one.
fn exercise(name: &str, text: &str) -> Option<Table> {
    let t = detect(text)?;
    if let Err(e) = well_formed(&t) {
        panic!("{name}: malformed table: {e}");
    }
    let r = render(&t, &ctx(text), 2500);
    assert!(!r.text.is_empty(), "{name}: empty view");
    let (before, after) = (est_tokens(text), est_tokens(&r.text));
    assert!(
        after * 100 <= before * 125 + 5000,
        "{name}: view ({after} tokens) is much bigger than input ({before})"
    );
    Some(t)
}

#[test]
fn random_text_never_panics() {
    let pieces = [
        ",",
        "\t",
        ";",
        "|",
        " ",
        "  ",
        "\n",
        "\r\n",
        "{",
        "}",
        "[",
        "]",
        "\"",
        ":",
        "-",
        "=",
        "+",
        "│",
        "┆",
        "─",
        "┌",
        "...",
        "…",
        "id",
        "name",
        "level",
        "123",
        "4.5",
        "true",
        "null",
        "2026-11-02",
        "10:00:00",
        "key=value",
        "\u{1b}[31m",
        "\u{1b}[0m",
        "é",
        "日本",
        "# c",
    ];
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for case in 0..4000 {
        let n = 1 + rng.below(400);
        let text: String = (0..n).map(|_| pieces[rng.below(pieces.len())]).collect();
        exercise(&format!("random case {case}"), &text);
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    for case in 0..800 {
        let n = rng.below(3000);
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        exercise(
            &format!("bytes case {case}"),
            &String::from_utf8_lossy(&bytes),
        );
    }
}

fn corpus_files() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let m: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let mut out = Vec::new();
    for kind in ["positive", "negative", "either"] {
        for e in m[kind].as_array().unwrap() {
            let f = e["file"].as_str().unwrap().to_string();
            let text = String::from_utf8_lossy(&fs::read(dir.join(&f)).unwrap()).to_string();
            out.push((f, text));
        }
    }
    out
}

fn mutate(rng: &mut Rng, text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    if lines.is_empty() {
        return String::new();
    }
    match rng.below(9) {
        0 => {
            let i = rng.below(lines.len());
            lines.remove(i);
        }
        1 => {
            let i = rng.below(lines.len());
            let l = lines[i].clone();
            lines.insert(i, l);
        }
        2 => {
            let i = rng.below(lines.len());
            lines[i].push_str("\u{1b}[31mx\u{1b}[0m");
        }
        3 => {
            let i = rng.below(lines.len());
            lines[i] = lines[i].replace(',', ";");
        }
        4 => {
            let (a, b) = (rng.below(lines.len()), rng.below(lines.len()));
            lines.swap(a, b);
        }
        5 => {
            let i = rng.below(lines.len());
            lines[i].clear();
        }
        6 => {
            let keep = 1 + rng.below(lines.len());
            lines.truncate(keep);
        }
        7 => {
            let i = rng.below(lines.len());
            let cut = lines[i]
                .chars()
                .take(rng.below(lines[i].chars().count() + 1))
                .collect();
            lines[i] = cut;
        }
        _ => lines.insert(0, "# header comment".into()),
    }
    lines.join("\n")
}

#[test]
fn mutated_real_files_never_panic_and_stay_well_formed() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let (mut runs, mut tables) = (0, 0);
    for (name, text) in corpus_files() {
        exercise(&name, &text);
        for round in 0..40 {
            let mut t = text.clone();
            for _ in 0..1 + rng.below(3) {
                t = mutate(&mut rng, &t);
            }
            runs += 1;
            if exercise(&format!("{name} mutation {round}"), &t).is_some() {
                tables += 1;
            }
        }
    }
    // The test must have teeth: many mutated inputs still have to be tables.
    assert!(
        tables * 4 >= runs,
        "only {tables} of {runs} mutated inputs were tables"
    );
}

#[test]
fn every_big_real_fixture_gets_a_much_smaller_summary() {
    let mut checked = 0;
    for (name, text) in corpus_files() {
        let Some(t) = detect(&text) else { continue };
        if t.rows.len() <= dtk::lean::LOSSLESS_ROWS || est_tokens(&text) < 1200 {
            continue;
        }
        let r = render(&t, &ctx(&text), 2500);
        let (before, after) = (est_tokens(&text), est_tokens(&r.text));
        assert!(after <= 2500, "{name}: summary is {after} tokens");
        assert!(
            after * 2 <= before,
            "{name}: summary {after} is not under half of input {before}"
        );
        checked += 1;
    }
    assert!(
        checked >= 10,
        "only {checked} fixtures were big enough; the corpus shrank"
    );
}
