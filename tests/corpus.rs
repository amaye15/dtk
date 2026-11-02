//! Gate G2: the detection matrix.
//!
//! `tests/corpus/manifest.json` lists real outputs of real tools (polars, pandas, sqlite3,
//! tabulate, rich, jq, git, gh) and what dtk must make of each:
//!
//! - `positive`: must be detected with this format, row count, column count and headers.
//! - `negative`: must be rejected. A false positive replaces real output with a summary.
//! - `either`: real output where either answer is fine, but a detected table must be well formed.
//!
//! See `tests/corpus/README.md` for how each file was made.

use dtk::table::{detect, Table};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

fn corpus() -> (PathBuf, Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let manifest =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    (dir, manifest)
}

fn read(dir: &std::path::Path, file: &str) -> String {
    let bytes = fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    String::from_utf8_lossy(&bytes).to_string()
}

/// A detected table must always be well formed.
fn check_well_formed(name: &str, t: &Table) {
    assert!(t.headers.len() >= 2, "{name}: fewer than two columns");
    let unique: HashSet<&String> = t.headers.iter().collect();
    assert_eq!(
        unique.len(),
        t.headers.len(),
        "{name}: duplicate headers {:?}",
        t.headers
    );
    assert!(
        t.headers.iter().all(|h| !h.trim().is_empty()),
        "{name}: empty header"
    );
    for (i, r) in t.rows.iter().enumerate() {
        assert_eq!(
            r.len(),
            t.headers.len(),
            "{name}: row {i} has {} cells, expected {}",
            r.len(),
            t.headers.len()
        );
    }
}

#[test]
fn every_positive_fixture_is_detected_as_expected() {
    let (dir, m) = corpus();
    let mut failures = Vec::new();
    for e in m["positive"].as_array().unwrap() {
        let file = e["file"].as_str().unwrap();
        let text = read(&dir, file);
        let Some(t) = detect(&text) else {
            failures.push(format!("{file}: not detected"));
            continue;
        };
        check_well_formed(file, &t);
        let want_fmt = e["format"].as_str().unwrap();
        if t.format.name() != want_fmt {
            failures.push(format!(
                "{file}: format {} (want {want_fmt})",
                t.format.name()
            ));
        }
        let (rows, cols) = (
            e["rows"].as_u64().unwrap() as usize,
            e["cols"].as_u64().unwrap() as usize,
        );
        if t.rows.len() != rows || t.headers.len() != cols {
            failures.push(format!(
                "{file}: {} rows x {} cols (want {rows} x {cols}); headers {:?}",
                t.rows.len(),
                t.headers.len(),
                t.headers
            ));
        }
        if let Some(h) = e.get("headers").and_then(Value::as_array) {
            let want: Vec<&str> = h.iter().map(|x| x.as_str().unwrap()).collect();
            if t.headers != want {
                failures.push(format!("{file}: headers {:?} (want {want:?})", t.headers));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn every_negative_fixture_is_rejected() {
    let (dir, m) = corpus();
    let mut failures = Vec::new();
    for e in m["negative"].as_array().unwrap() {
        let file = e["file"].as_str().unwrap();
        if let Some(t) = detect(&read(&dir, file)) {
            failures.push(format!(
                "{file}: wrongly detected as {} ({} rows x {} cols, headers {:?})",
                t.format.name(),
                t.rows.len(),
                t.headers.len(),
                t.headers
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn ambiguous_real_outputs_never_produce_a_malformed_table() {
    let (dir, m) = corpus();
    for e in m["either"].as_array().unwrap() {
        let file = e["file"].as_str().unwrap();
        if let Some(t) = detect(&read(&dir, file)) {
            check_well_formed(file, &t);
        }
    }
}
