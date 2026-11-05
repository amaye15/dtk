//! `dtk skeleton`: the layout of an output without its values, for a bug report.
//!
//! The promise is that no letter and no digit of the original is in the result. The test checks
//! it on a planted secret and on every fixture of the corpus.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn skeleton(args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .arg("skeleton")
        .args(args)
        .env("PATH", "/usr/bin:/bin")
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
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The skeleton itself: what follows the three lines that dtk adds above it.
fn body(out: &str) -> &str {
    out.split_once("\n---\n").expect("the separator line").1
}

#[test]
fn a_planted_secret_never_comes_out() {
    let mut text = String::from("id,customer,card,amount\n");
    for i in 0..30 {
        text.push_str(&format!(
            "{i},Ann Hunter-{i},4111-1111-1111-{:04},{}.50\n",
            1000 + i,
            i * 7
        ));
    }
    text.push_str("99,hunter2-SECRET-TOKEN,sk_live_abc123XYZ,0.00\n");
    let (code, out, _) = skeleton(&[], &text);
    assert_eq!(code, 0);
    for secret in [
        "hunter2", "SECRET", "TOKEN", "sk_live", "abc123", "4111", "Ann", "customer",
    ] {
        assert!(!out.contains(secret), "`{secret}` is in\n{out}");
    }
    // The header says what dtk makes of the output, which is the point of sending it.
    assert!(
        out.contains("dtk reads this output as: csv, 31 rows x 4 cols"),
        "{out}"
    );
    // The layout is there.
    assert!(
        body(&out)
            .starts_with("aa,aaaaaaaa,aaaa,aaaaaa\n9,Aaa Aaaaaa-9,9999-9999-9999-9999,9.99\n"),
        "{out}"
    );
}

#[test]
fn keep_header_keeps_the_first_line_and_only_that() {
    let (_, out, _) = skeleton(&["--keep-header"], "order_id,customer\n5042,Ann\n");
    assert_eq!(body(&out), "order_id,customer\n9999,Aaa\n");
    assert!(out.contains("except the first line"), "{out}");
}

#[test]
fn head_and_tail_limit_the_size() {
    let text: String = (0..500).map(|i| format!("event {i} ok\n")).collect();
    let (_, out, _) = skeleton(&["--head", "4", "--tail", "2"], &text);
    assert!(body(&out).contains("... 494 lines left out ..."), "{out}");
    assert_eq!(body(&out).lines().count(), 4 + 1 + 2);
}

#[test]
fn a_bad_call_is_refused_with_a_hint() {
    let (code, _, err) = skeleton(&["--head"], "");
    assert_eq!(code, 2);
    assert!(err.contains("usage: dtk skeleton"), "{err}");
    let (code, _, _) = skeleton(&["a.txt", "b.txt"], "");
    assert_eq!(code, 2);
    let (code, _, _) = skeleton(&["/no/such/file"], "");
    assert_eq!(code, 1);
}

/// What is left of a skeleton when dtk's own markers are taken out: the line that counts the lines
/// left out, and the count of characters cut from a long line. Those hold numbers that dtk wrote.
fn without_markers(skeleton: &str) -> String {
    skeleton
        .lines()
        .filter(|l| !(l.starts_with("... ") && l.ends_with(" lines left out ...")))
        .map(|l| l.split(" ...(+").next().unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// For every fixture: after the lines that dtk adds, no letter or digit is left except the three
/// that stand for a class.
#[test]
fn nothing_but_classes_is_left_of_any_fixture() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut checked = 0;
    for sub in [
        "positive",
        "negative",
        "wrapped",
        "gh",
        "frames",
        "logs",
        "pages",
        "documents",
    ] {
        for e in fs::read_dir(dir.join(sub)).unwrap().flatten() {
            let text = String::from_utf8_lossy(&fs::read(e.path()).unwrap()).to_string();
            let (code, out, _) = skeleton(&[], &text);
            assert_eq!(code, 0);
            for c in without_markers(body(&out)).chars() {
                assert!(
                    !c.is_alphanumeric() || matches!(c, 'a' | 'A' | '9'),
                    "`{c}` is left of {}",
                    e.path().display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "only {checked} fixtures were checked");
}
