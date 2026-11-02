//! Gate G5: dtk against the REAL rtk binary, in a throwaway HOME.
//!
//! Skipped (with a message) when `rtk` is not on PATH. CI installs rtk, so it runs there.
//! These tests exist because rtk re-writes `~/.claude/settings.json` on `rtk init -g`, and dtk
//! must survive every order of install, re-install and uninstall.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn rtk_available() -> bool {
    Command::new("rtk")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

macro_rules! need_rtk {
    () => {
        if !rtk_available() {
            eprintln!("SKIPPED: rtk is not installed");
            return;
        }
    };
}

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-real-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn rtk(home: &Path, args: &[&str]) -> String {
    let out = Command::new("rtk")
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn settings(home: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap()).unwrap()
}

fn post(home: &Path) -> Vec<Value> {
    settings(home)["hooks"]["PostToolUse"]
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

/// Turns on rtk's PostToolUse pipeline for this HOME (both platform paths).
fn enable_posthook(home: &Path) {
    let text = "schema_version = 1\n\n[pipeline]\nextends = \"rtk:posthook\"\n";
    for dir in [
        home.join("Library/Application Support/rtk"),
        home.join(".config/rtk"),
    ] {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pipeline.toml"), text).unwrap();
    }
}

fn is_chain(c: &str) -> bool {
    c.ends_with("dtk hook claude --then rtk hook claude")
}

fn is_placeholder(c: &str) -> bool {
    c.contains("/dtk/shim/rtk") && c.ends_with("hook claude")
}

#[test]
fn rtk_init_run_twice_after_dtk_adds_no_second_hook() {
    need_rtk!();
    let home = temp_home("twice");
    dtk(&home, &["init", "--global"]);
    for _ in 0..2 {
        rtk(&home, &["init", "-g", "--auto-patch"]);
    }
    let p = post(&home);
    assert_eq!(p.len(), 1, "one PostToolUse entry, not two: {p:?}");
    let cmds = commands(&p[0]);
    assert!(
        is_chain(&cmds[0]) && cmds.iter().any(|c| is_placeholder(c)),
        "{cmds:?}"
    );
    let pre = settings(&home)["hooks"]["PreToolUse"].clone();
    assert!(
        pre.to_string().contains("rtk hook claude"),
        "rtk still gets its own PreToolUse hook: {pre}"
    );
}

#[test]
fn rtk_first_then_dtk_takes_over_and_rtk_init_stays_quiet() {
    need_rtk!();
    let home = temp_home("rtkfirst");
    enable_posthook(&home);
    rtk(&home, &["init", "-g", "--auto-patch"]);
    assert_eq!(
        post(&home).len(),
        1,
        "rtk registered its own PostToolUse hook"
    );
    let out = dtk(&home, &["init", "--global"]);
    assert!(out.contains("took over the rtk PostToolUse hook"), "{out}");
    assert_eq!(post(&home).len(), 1);
    rtk(&home, &["init", "-g", "--auto-patch"]);
    let p = post(&home);
    assert_eq!(p.len(), 1, "rtk init must not add a second hook: {p:?}");
    assert!(
        p[0]["matcher"].as_str().unwrap().contains("Grep"),
        "rtk's matcher is kept: {}",
        p[0]["matcher"]
    );
    let show = rtk(&home, &["init", "--show"]);
    assert!(
        show.contains("PreToolUse + PostToolUse"),
        "rtk is satisfied: {show}"
    );
    let validate = rtk(&home, &["pipeline", "validate"]);
    assert!(!validate.contains("hook needed"), "{validate}");
}

#[test]
fn rtk_uninstall_removes_only_the_placeholder() {
    need_rtk!();
    let home = temp_home("rtkuninstall");
    dtk(&home, &["init", "--global"]);
    rtk(&home, &["init", "-g", "--auto-patch"]);
    rtk(&home, &["init", "-g", "--uninstall"]);
    let p = post(&home);
    assert_eq!(p.len(), 1, "dtk's entry survives: {p:?}");
    let cmds = commands(&p[0]);
    assert!(cmds.iter().any(|c| is_chain(c)), "{cmds:?}");
    assert!(
        !cmds.iter().any(|c| is_placeholder(c)),
        "rtk removed the placeholder: {cmds:?}"
    );
    let doctor = dtk(&home, &["doctor"]);
    assert!(
        doctor.contains("[warn] dtk's placeholder for rtk is missing"),
        "{doctor}"
    );
    dtk(&home, &["init", "--global"]);
    assert!(
        commands(&post(&home)[0]).iter().any(|c| is_placeholder(c)),
        "init puts the placeholder back"
    );
}

#[test]
fn dtk_uninstall_gives_rtks_own_hook_back() {
    need_rtk!();
    let home = temp_home("restore");
    enable_posthook(&home);
    rtk(&home, &["init", "-g", "--auto-patch"]);
    let original = post(&home);
    dtk(&home, &["init", "--global"]);
    let out = dtk(&home, &["init", "--uninstall"]);
    assert!(out.contains("restored the rtk PostToolUse hook"), "{out}");
    assert_eq!(
        post(&home),
        original,
        "rtk's entry is back exactly as rtk wrote it"
    );
}

#[test]
fn dtk_uninstall_does_not_resurrect_a_hook_rtk_removed() {
    need_rtk!();
    let home = temp_home("noresurrect");
    enable_posthook(&home);
    rtk(&home, &["init", "-g", "--auto-patch"]);
    dtk(&home, &["init", "--global"]);
    rtk(&home, &["init", "-g", "--uninstall"]);
    dtk(&home, &["init", "--uninstall"]);
    assert!(
        post(&home).is_empty(),
        "nothing of rtk or dtk is left: {:?}",
        post(&home)
    );
}

#[test]
fn enabling_the_posthook_pipeline_later_widens_dtks_matcher_through_rtk() {
    need_rtk!();
    let home = temp_home("widen");
    dtk(&home, &["init", "--global"]);
    assert_eq!(post(&home)[0]["matcher"], "Bash|Read");
    enable_posthook(&home);
    rtk(&home, &["init", "-g", "--auto-patch"]);
    let p = post(&home);
    assert_eq!(p.len(), 1);
    let m = p[0]["matcher"].as_str().unwrap().to_string();
    assert!(m.contains("Grep") && m.contains("WebFetch"), "{m}");
    dtk(&home, &["init", "--global"]);
    assert_eq!(
        post(&home)[0]["matcher"],
        m.as_str(),
        "dtk keeps what rtk widened"
    );
    let out = dtk(&home, &["init", "--uninstall"]);
    assert!(out.contains("restored the rtk PostToolUse hook"), "{out}");
    assert_eq!(post(&home)[0]["matcher"], m.as_str());
    assert_eq!(commands(&post(&home)[0]), ["rtk hook claude"]);
}

fn event(tool: &str, command: &str, response: Value) -> String {
    json!({
        "hook_event_name": "PostToolUse", "tool_name": tool, "cwd": "/tmp",
        "tool_input": {"command": command}, "tool_response": response,
    })
    .to_string()
}

fn run_chain(home: &Path, ev: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(["hook", "claude", "--then", "rtk", "hook", "claude"])
        .env("HOME", home)
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
    String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).to_string()
}

#[test]
fn the_real_chain_gives_tables_to_dtk_and_everything_else_to_rtk() {
    need_rtk!();
    let home = temp_home("chain");
    enable_posthook(&home);
    // A table: dtk answers.
    let rows: String = (0..300)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"level\":\"{}\",\"ms\":{}}}\n",
                ["info", "warn"][i % 2],
                i * 3
            )
        })
        .collect();
    let table = run_chain(
        &home,
        &event(
            "Bash",
            "cat events.jsonl",
            json!({"stdout": rows, "stderr": ""}),
        ),
    );
    assert!(table.contains("[dtk] jsonl: 300 rows"), "{table}");
    // Not a table, from a command rtk does not rewrite: rtk's posthook pipeline answers.
    let text: String = (0..800)
        .map(|i| format!("line {i}: value={} status=ok\n", i * 7))
        .collect();
    let other = run_chain(
        &home,
        &event(
            "Bash",
            "/bin/cat notes.txt",
            json!({"stdout": text, "stderr": ""}),
        ),
    );
    assert!(
        other.contains("[full output:"),
        "rtk's recovery pointer is there: {other}"
    );
    assert!(!other.contains("[dtk]"), "{other}");
}
