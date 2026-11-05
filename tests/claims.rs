//! Gate G7: a summary only claims what a reader can reach.
//!
//! A summary says that all of the data is on disk, names the file, and suggests commands to read
//! more. Each of those is a promise. Until 0.18.0 a small table that got the statistics summary
//! said that the full data was on disk, and nothing was saved: a promise that nothing kept.
//! `tests/retention.rs` could not see it, because it calls `render` alone, and the file is saved
//! one level up, in `hook::build`.
//!
//! This test runs the real binary on every fixture of the corpus and checks what the output
//! promises:
//!
//! - "the full data (or document) is on disk" names a file, and the file exists;
//! - every path in dtk's store that the output names exists;
//! - `N rows x M cols` is what the saved CSV (or the CSV that the output shows) holds;
//! - every `dtk` command in `drill:` and `more:` runs and answers;
//! - a page that "is saved as ID" can be read with `dtk html ID`.
//!
//! The audit has tests of its own below: it must find each kind of broken promise.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_dtk");

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-claims-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, args: &[&str], stdin: &str) -> (i32, String) {
    let mut child = Command::new(BIN)
        .args(args)
        .env("HOME", home)
        // A fixed PATH keeps the real rtk out of these tests.
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DTK_MODE")
        .env_remove("DTK_DISABLED")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

/// What the hook answers for the output of `command`, or `None` when it leaves it alone.
fn hook(home: &Path, command: &str, text: &str) -> Option<String> {
    let ev = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash",
                    "tool_input": {"command": command},
                    "tool_response": {"stdout": text, "stderr": "", "interrupted": false, "isImage": false}})
    .to_string();
    let (code, out) = dtk(home, &["hook", "claude"], &ev);
    assert_eq!(code, 0, "the hook failed on `{command}`");
    let v: Value = serde_json::from_str(&out).ok()?;
    Some(
        v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()?
            .to_string(),
    )
}

/// The words of a command line: single quotes hold a word together, and `'\''` is a quote.
fn shell_split(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' if !quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_word = true;
                }
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// A word that names a file in dtk's store (`.../dtk/store/abc.csv`), without its quotes.
fn store_path(word: &str) -> Option<String> {
    let w = word.trim_matches(['\'', '"', '(', ')', ',', ';']);
    w.replace('\\', "/")
        .contains("/dtk/store/")
        .then(|| w.to_string())
}

/// The numbers in `[dtk] NAME: 300 rows x 7 cols, ...` or `[dtk] NAME as csv: 40 rows x 5 cols, ...`.
fn counts(first_line: &str) -> Option<(usize, usize)> {
    let (left, right) = first_line.split_once(" rows x ")?;
    let rows = left.rsplit(' ').next()?.parse().ok()?;
    let cols = right.split(' ').next()?.parse().ok()?;
    Some((rows, cols))
}

fn csv_shape(text: &str) -> (usize, usize) {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let cols = rdr.headers().map(|h| h.len()).unwrap_or(0);
    (rdr.records().count(), cols)
}

/// Everything that is wrong with the promises in `summary`. Empty when it keeps all of them.
fn audit(summary: &str, home: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    let first = summary.lines().next().unwrap_or("");

    // 1. "The full data is on disk" names a file.
    let paths: Vec<String> = summary.split_whitespace().filter_map(store_path).collect();
    let claims_disk = summary.contains("is on disk");
    if claims_disk && paths.is_empty() {
        bad.push("says that the full data is on disk, and names no file".into());
    }
    for p in &paths {
        if !Path::new(p).is_file() {
            bad.push(format!("names {p}, which does not exist"));
        }
    }

    // 2. The numbers in the first line are what the saved CSV holds.
    if let (Some((rows, cols)), Some(data)) = (
        counts(first),
        summary.lines().find_map(|l| l.strip_prefix("data: ")),
    ) {
        let path = data.strip_suffix(" (csv, header row)").unwrap_or(data);
        let path = path.trim_matches('\'');
        match fs::read_to_string(path) {
            Ok(csv) => {
                let (r, c) = csv_shape(&csv);
                if (r, c) != (rows, cols) {
                    bad.push(format!(
                        "says {rows} rows x {cols} cols, and the saved CSV has {r} x {c}"
                    ));
                }
            }
            Err(e) => bad.push(format!("cannot read the data file {path}: {e}")),
        }
    }

    // 3. A table shown whole holds the rows it says.
    if first.contains(" as csv: ") && first.contains("nothing dropped") {
        if let Some((rows, cols)) = counts(first) {
            let body: Vec<&str> = summary
                .lines()
                .skip(1)
                .take_while(|l| !l.starts_with("note:"))
                .collect();
            let (r, c) = csv_shape(&body.join("\n"));
            if (r, c) != (rows, cols) {
                bad.push(format!(
                    "says {rows} rows x {cols} cols, nothing dropped, and shows {r} x {c}"
                ));
            }
        }
    }

    // 4. A page that is saved can be read.
    if let Some(id) = summary.split("is saved as ").nth(1).map(|s| {
        s.trim_end_matches(['.', ' ', '\n'])
            .split(['.', '\n'])
            .next()
            .unwrap_or("")
    }) {
        let (code, out) = dtk(home, &["html", id, "headings"], "");
        if code != 0 || out.trim().is_empty() {
            bad.push(format!(
                "`dtk html {id} headings` does not answer (exit {code})"
            ));
        }
    }

    // 5. Every `dtk` command that the output suggests runs.
    for line in summary.lines() {
        let Some(list) = line.strip_prefix("drill: ") else {
            continue;
        };
        for segment in list.split("  |  ") {
            let words = shell_split(segment.trim());
            let Some(first_word) = words.first() else {
                continue;
            };
            let name = first_word.rsplit(['/', '\\']).next().unwrap_or("");
            if !name.starts_with("dtk") {
                // `jaq` and the like: not dtk's own, and not always installed.
                continue;
            }
            let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
            let (code, out) = dtk(home, &args, "");
            if code != 0 || out.trim().is_empty() {
                bad.push(format!(
                    "`{}` does not answer (exit {code})",
                    segment.trim()
                ));
            }
        }
    }
    bad
}

// ---- the fixtures of the corpus -----------------------------------------------------------------

fn fixtures() -> Vec<(String, String, String)> {
    let dir = corpus_dir();
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let mut out: Vec<(String, String, String)> = Vec::new();
    let read = |file: &str| String::from_utf8_lossy(&fs::read(dir.join(file)).unwrap()).to_string();
    let mut seen = std::collections::HashSet::new();
    for key in ["positive", "wrapped", "gh"] {
        for e in manifest[key].as_array().unwrap() {
            let file = e["file"].as_str().unwrap().to_string();
            let command = e["command"].as_str().unwrap_or("mytool --run").to_string();
            if seen.insert(file.clone()) {
                out.push((file.clone(), command, read(&file)));
            }
        }
    }
    // Pages, documents, logs and frames that the manifest does not list.
    for (sub, command) in [
        ("pages", "curl -s https://example.com/docs"),
        ("documents", "mytool --run"),
        ("logs", "mytool --run"),
        ("frames", "mytool --run"),
    ] {
        let mut names: Vec<_> = fs::read_dir(dir.join(sub))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| !n.ends_with(".expected.csv"))
            .collect();
        names.sort();
        for n in names {
            let file = format!("{sub}/{n}");
            if seen.insert(file.clone()) {
                out.push((file.clone(), command.to_string(), read(&file)));
            }
        }
    }
    out
}

#[test]
fn every_summary_of_the_corpus_keeps_its_promises() {
    let mut audited = 0;
    let mut broken: Vec<String> = Vec::new();
    for (i, (file, command, text)) in fixtures().into_iter().enumerate() {
        let home = temp_home(&format!("fx{i}"));
        let Some(summary) = hook(&home, &command, &text) else {
            continue;
        };
        audited += 1;
        for problem in audit(&summary, &home) {
            broken.push(format!("{file}: {problem}"));
        }
    }
    assert!(
        audited >= 50,
        "only {audited} fixtures were summarized: the test proves too little"
    );
    assert!(broken.is_empty(), "broken promises:\n{}", broken.join("\n"));
}

// ---- the audit itself must find each kind of broken promise -------------------------------------

fn store(home: &Path, name: &str, text: &str) -> String {
    let dir = home.join(".local/share/dtk/store");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, text).unwrap();
    p.display().to_string()
}

#[test]
fn the_audit_finds_data_that_is_on_disk_in_words_only() {
    // This is what 0.17.0 printed for a wide table of 60 rows.
    let home = temp_home("teeth-words");
    let summary = "[dtk] table: 60 rows x 19 cols, 10.5KB (statistics over all rows; the full data is on disk)\ncols: a:int b:str\n";
    let bad = audit(summary, &home);
    assert!(bad.iter().any(|b| b.contains("names no file")), "{bad:?}");
}

#[test]
fn the_audit_finds_a_file_that_is_not_there() {
    let home = temp_home("teeth-file");
    let gone = home.join(".local/share/dtk/store/gone.csv");
    let summary = format!(
        "[dtk] table: 5 rows x 2 cols, 1KB (the full data is on disk)\ndata: {} (csv, header row)\n",
        gone.display()
    );
    let bad = audit(&summary, &home);
    assert!(bad.iter().any(|b| b.contains("does not exist")), "{bad:?}");
}

#[test]
fn the_audit_finds_counts_that_the_saved_csv_does_not_have() {
    let home = temp_home("teeth-counts");
    let p = store(&home, "x.csv", "a,b\n1,2\n3,4\n5,6\n");
    let summary = format!(
        "[dtk] table: 30 rows x 2 cols, 1KB (the full data is on disk)\ndata: {p} (csv, header row)\n"
    );
    let bad = audit(&summary, &home);
    assert!(
        bad.iter().any(|b| b.contains("saved CSV has 3 x 2")),
        "{bad:?}"
    );
    // The same file with the right numbers keeps the promise.
    let ok = format!(
        "[dtk] table: 3 rows x 2 cols, 1KB (the full data is on disk)\ndata: {p} (csv, header row)\n"
    );
    assert!(audit(&ok, &home).is_empty());
}

#[test]
fn the_audit_finds_a_table_that_shows_fewer_rows_than_it_says() {
    let home = temp_home("teeth-whole");
    let summary = "[dtk] csv as csv: 4 rows x 2 cols, nothing dropped\na,b\n1,2\n3,4\n";
    let bad = audit(summary, &home);
    assert!(bad.iter().any(|b| b.contains("shows 2 x 2")), "{bad:?}");
}

#[test]
fn the_audit_finds_a_command_that_does_not_answer() {
    let home = temp_home("teeth-drill");
    let summary = format!(
        "[dtk] table: 3 rows x 2 cols, 1KB (the full data is on disk)\ndrill: {BIN} where deadbeef0000 a = 1  |  (also rows: dtk rows --help)\n"
    );
    let bad = audit(&summary, &home);
    assert!(bad.iter().any(|b| b.contains("does not answer")), "{bad:?}");
}

#[test]
fn words_in_single_quotes_stay_together() {
    assert_eq!(
        shell_split("dtk where abc name = 'it'\\''s here'  x"),
        ["dtk", "where", "abc", "name", "=", "it's here", "x"]
    );
}
