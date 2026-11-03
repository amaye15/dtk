//! The log digest: what it must say about real build and test output, and what it must never
//! be made of. The facts are counted in the test with plain string work, not with the digest's
//! own code.

use dtk::digest::digest;
use dtk::lean::est_tokens;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn log(name: &str) -> String {
    fs::read_to_string(root().join("tests/corpus/logs").join(name)).unwrap()
}

fn line_numbers(text: &str, pred: impl Fn(&str) -> bool) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| pred(l))
        .map(|(i, _)| i + 1)
        .collect()
}

#[test]
fn a_pytest_run_says_how_many_failed_and_where() {
    let text = log("pytest_verbose_7_failures.log");
    let d = digest(&text, Some("/tmp/pytest.log")).expect("a test log");
    assert!(d.contains("7 failed, 593 passed"), "{d}");
    // Every failing test is findable: its line in the full output is in the digest.
    let at = line_numbers(&text, |l| {
        l.contains("test_order_total[") && l.contains(" FAILED ")
    });
    assert_eq!(at.len(), 7, "the log has seven failures");
    for n in at {
        assert!(
            d.contains(&n.to_string()),
            "line {n} of a failure is not in the digest:\n{d}"
        );
    }
    assert!(
        d.contains("/tmp/pytest.log"),
        "the saved file is named: {d}"
    );
    assert!(
        est_tokens(&d) * 10 < est_tokens(&text),
        "{} -> {}",
        est_tokens(&text),
        est_tokens(&d)
    );
}

#[test]
fn a_cargo_test_run_names_the_failures_and_the_totals() {
    let text = log("cargo_test_4_failures.log");
    let d = digest(&text, None).expect("a test log");
    assert!(d.contains("396 passed; 4 failed"), "{d}");
    for name in [
        "check_item_031",
        "check_item_120",
        "check_item_222",
        "check_item_361",
    ] {
        assert!(d.contains(name), "{name} is not in the digest:\n{d}");
    }
}

#[test]
fn a_compiler_log_counts_each_kind_of_error() {
    let text = log("rustc_80_errors.log");
    let d = digest(&text, None).expect("compiler output");
    for code in ["E0425", "E0308", "E0277"] {
        let n = text
            .lines()
            .filter(|l| l.starts_with(&format!("error[{code}]")))
            .count();
        assert!(n > 10, "the fixture has many {code}");
        assert!(d.contains(&format!("error[{code}]")), "{code}: {d}");
        assert!(d.contains(&format!("(x{n};")), "x{n} for {code}: {d}");
    }
    assert!(d.contains("due to 106 previous errors"), "{d}");
    assert!(d.contains("in files: src/lib.rs"), "{d}");
    // The place under the first message is shown.
    assert!(d.contains("--> src/lib.rs:2:28"), "{d}");
}

#[test]
fn nothing_that_is_not_a_log_gets_a_digest() {
    let mut files: Vec<PathBuf> = vec![root().join("README.md"), root().join("CHANGELOG.md")];
    for dir in [
        "src",
        "docs",
        "tests/corpus/negative",
        "tests/corpus/documents",
        "tests",
    ] {
        if let Ok(rd) = fs::read_dir(root().join(dir)) {
            files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_file()));
        }
    }
    let mut checked = 0;
    for f in files {
        let Ok(text) = fs::read_to_string(&f) else {
            continue;
        };
        checked += 1;
        assert!(
            digest(&text, None).is_none(),
            "{} was taken for a log",
            f.display()
        );
    }
    assert!(checked > 40, "only {checked} files were checked");
}
