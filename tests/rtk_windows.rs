//! Windows: dtk next to the real rtk.
//!
//! rtk finds the Windows profile through the operating system, so unlike the macOS and Linux
//! tests (`rtk_real.rs`) this cannot run in a throwaway home. It changes the real profile, so it
//! is ignored by default. CI runs it on a throwaway runner:
//!
//!     cargo test --test rtk_windows -- --ignored

#![cfg(windows)]

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn profile() -> PathBuf {
    PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"))
}

fn settings() -> Value {
    serde_json::from_str(&fs::read_to_string(profile().join(".claude/settings.json")).unwrap())
        .unwrap()
}

fn dtk(args: &[&str]) -> (i32, String) {
    // No HOME: dtk has to find the profile through USERPROFILE.
    let o = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(args)
        .env_remove("HOME")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

fn rtk(args: &[&str]) -> String {
    let o = Command::new("rtk")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn post() -> Vec<Value> {
    settings()["hooks"]["PostToolUse"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn commands(entry: &Value) -> Vec<String> {
    entry["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["command"].as_str().unwrap().to_string())
        .collect()
}

fn placeholder(c: &str) -> bool {
    c.replace('\\', "/")
        .replace('"', "")
        .contains("/dtk/shim/rtk.exe")
        && c.ends_with("hook claude")
}

#[test]
#[ignore = "changes the real profile; CI only"]
fn dtk_and_the_real_rtk_coexist_on_windows() {
    fs::create_dir_all(profile().join(".claude")).unwrap();
    fs::write(profile().join(".claude/settings.json"), "{}").unwrap();

    let (code, out) = dtk(&["init", "--global"]);
    assert_eq!(code, 0, "{out}");
    let p = post();
    assert_eq!(p.len(), 1, "{p:?}");
    let cmds = commands(&p[0]);
    assert!(
        cmds[0].contains("dtk.exe") && cmds[0].ends_with(" hook claude --then rtk hook claude"),
        "{cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| placeholder(c)),
        "the placeholder is rtk.exe in dtk's folder: {cmds:?}"
    );
    assert!(!cmds[0].contains('\\'), "forward slashes only: {}", cmds[0]);

    // The placeholder, run the way a hook runs it, does nothing.
    let shim = cmds
        .iter()
        .find(|c| placeholder(c))
        .unwrap()
        .trim_end_matches(" hook claude")
        .trim_matches('"')
        .to_string();
    let mut child = Command::new(&shim)
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(b"{}");
    let o = child.wait_with_output().unwrap();
    assert!(
        o.status.success() && o.stdout.is_empty() && o.stderr.is_empty(),
        "placeholder: {o:?}"
    );

    // rtk init, twice: it must add its PreToolUse hook and nothing else.
    for _ in 0..2 {
        rtk(&["init", "-g", "--auto-patch"]);
    }
    assert_eq!(
        post().len(),
        1,
        "rtk init must not add a second PostToolUse hook: {:?}",
        post()
    );
    assert!(
        settings()["hooks"]["PreToolUse"]
            .to_string()
            .contains("rtk hook claude"),
        "{}",
        settings()
    );
    let (_, doctor) = dtk(&["doctor"]);
    assert!(doctor.contains("[ok] dtk is chained to rtk"), "{doctor}");

    // rtk uninstall removes the placeholder and leaves dtk's chain.
    rtk(&["init", "-g", "--uninstall"]);
    let cmds = commands(&post()[0]);
    assert!(
        cmds.iter().any(|c| c.contains("--then rtk hook claude"))
            && !cmds.iter().any(|c| placeholder(c)),
        "{cmds:?}"
    );

    // The chain, with the real rtk.exe: a table is dtk's, and dtk answers.
    let rows: String = (0..300)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"level\":\"{}\",\"ms\":{}}}\n",
                ["info", "warn"][i % 2],
                i * 3
            )
        })
        .collect();
    let ev = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "cat e.jsonl"},
        "tool_response": {"stdout": rows, "stderr": "", "interrupted": false, "isImage": false}})
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(["hook", "claude", "--then", "rtk", "hook", "claude"])
        .env_remove("HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(ev.as_bytes())
        .unwrap();
    let out = String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).to_string();
    assert!(out.contains("[dtk] jsonl: 300 rows"), "{out}");

    // Reinstall, then uninstall: nothing of dtk is left.
    assert_eq!(dtk(&["init", "--global"]).0, 0);
    assert_eq!(dtk(&["init", "--uninstall"]).0, 0);
    assert!(!settings().to_string().contains("dtk"), "{}", settings());
}
