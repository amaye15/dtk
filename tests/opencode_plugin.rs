//! The OpenCode plugin, run with Node as OpenCode would call it (see `tests/opencode_plugin.mjs`).
//!
//! It needs Node 22.18 or later, which runs TypeScript as it is. Without it the test says so and
//! passes: the real OpenCode is tried by `scripts/opencode-test.sh`.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[test]
fn the_plugin_changes_a_result_only_when_dtk_answers() {
    let version = Command::new("node")
        .arg("-p")
        .arg("process.features.typescript || 'no'")
        .output();
    let Ok(v) = version else {
        eprintln!("SKIPPED: node is not installed");
        return;
    };
    let strips = String::from_utf8_lossy(&v.stdout).trim().to_string();
    if strips != "strip" && strips != "transform" {
        eprintln!("SKIPPED: this node does not run TypeScript ({strips})");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("node")
        .arg("--no-warnings")
        .arg(root.join("tests/opencode_plugin.mjs"))
        .env("DTK_BIN", env!("CARGO_BIN_EXE_dtk"))
        .env("DTK_ROOT", root)
        .env_remove("DTK_DISABLED")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.lines().filter(|l| l.starts_with("ok")).count() >= 18,
        "{text}"
    );
}
