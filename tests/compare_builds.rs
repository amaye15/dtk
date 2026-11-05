//! `scripts/compare-builds.sh` reports what two builds make differently of the same files, and
//! fails on a difference that was not expected.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn work(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-compare-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("files")).unwrap();
    dir.canonicalize().unwrap()
}

/// A stand-in for `dtk explain FILE --command CMD` that says what it is told to say for a file.
fn fake(dir: &Path, name: &str, answers: &[(&str, &str)]) -> PathBuf {
    let mut body = String::from("#!/bin/sh\nf=\"$2\"\ncase \"$f\" in\n");
    for (file, answer) in answers {
        body.push_str(&format!(
            "  *{file}) printf 'detected:  {}\\ndecision:  {} (the summary is 50%% smaller)\\n' ;;\n",
            answer.split_once('/').unwrap().0.trim(),
            answer.split_once('/').unwrap().1.trim()
        ));
    }
    body.push_str(
        "  *) printf 'detected:  nothing\\ndecision:  PASS THROUGH (not a table)\\n' ;;\nesac\n",
    );
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/compare-builds.sh")
}

fn text(n: usize) -> String {
    "some text that is long enough to be compared\n".repeat(n)
}

#[test]
fn it_lists_a_difference_and_fails_unless_it_was_expected() {
    let dir = work("diff");
    for n in ["a.txt", "b.txt", "c.txt"] {
        fs::write(dir.join("files").join(n), text(60)).unwrap();
    }
    let old = fake(&dir, "old", &[("b.txt", "nothing / PASS THROUGH")]);
    let new = fake(&dir, "new", &[("b.txt", "csv, 300 rows x 7 cols / APPLY")]);
    let files = dir.join("files");

    let run = |extra: &[&str]| {
        let out = Command::new("bash")
            .arg(script())
            .arg(&old)
            .arg(&new)
            .args(extra)
            .arg(&files)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8_lossy(&out.stdout).to_string(),
        )
    };

    let (code, out) = run(&[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("b.txt"), "{out}");
    assert!(out.contains("old: nothing / PASS THROUGH"), "{out}");
    assert!(out.contains("new: csv, 300 rows x 7 cols / APPLY"), "{out}");
    assert!(
        out.contains("3 text files compared: 2 the same, 1 different (1 not expected)"),
        "{out}"
    );

    // A difference that matches --allow is listed and does not fail.
    let (code, out) = run(&["--allow", "b\\.txt$"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("ok "), "{out}");
    assert!(out.contains("(0 not expected)"), "{out}");
}

#[test]
fn it_passes_when_the_builds_agree_and_skips_small_and_binary_files() {
    let dir = work("same");
    fs::write(dir.join("files/a.txt"), text(60)).unwrap();
    fs::write(dir.join("files/tiny.txt"), "small").unwrap();
    fs::write(dir.join("files/blob.bin"), vec![0u8; 5000]).unwrap();
    let old = fake(&dir, "old", &[]);
    let new = fake(&dir, "new", &[]);
    let out = Command::new("bash")
        .arg(script())
        .arg(&old)
        .arg(&new)
        .arg(dir.join("files"))
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{s}");
    assert!(
        s.contains("1 text files compared: 1 the same, 0 different"),
        "{s}"
    );
}
