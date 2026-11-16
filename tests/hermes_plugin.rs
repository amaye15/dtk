//! The Hermes plugin, run with Python as Hermes would call it (see `tests/hermes_plugin.py`).
//!
//! Without Python 3 the test says so and passes. The real Hermes was not run.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[test]
fn the_plugin_changes_a_result_only_when_dtk_answers() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("SKIPPED: python3 is not installed");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("python3")
        .arg("-I")
        .arg(root.join("tests/hermes_plugin.py"))
        .env("DTK_BIN", env!("CARGO_BIN_EXE_dtk"))
        .env("DTK_ROOT", root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.lines().filter(|l| l.starts_with("ok")).count() >= 16,
        "{text}"
    );
}
