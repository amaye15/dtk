//! Runs the real binary against simulated Claude Code events, with a throwaway HOME.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-test-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn jsonl(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "{{\"ts\":\"2026-11-02T10:00:{:02}Z\",\"level\":\"{}\",\"msg\":\"event number {}\",\"ms\":{}}}\n",
                i % 60,
                ["info", "info", "warn", "error"][i % 4],
                i,
                i * 7
            )
        })
        .collect()
}

fn event(tool: &str, response: Value) -> String {
    json!({
        "hook_event_name": "PostToolUse",
        "tool_name": tool,
        "tool_input": {"command": "cat data.jsonl"},
        "tool_response": response,
    })
    .to_string()
}

fn run(home: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dtk"));
    // A fixed PATH keeps the real rtk out of these tests.
    cmd.args(args)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DTK_MODE")
        .env_remove("DTK_DISABLED");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
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

#[test]
fn replaces_a_mid_size_table_and_keeps_other_fields() {
    let home = temp_home("mid");
    let resp =
        json!({"stdout": jsonl(300), "stderr": "warn: x", "interrupted": false, "isImage": false});
    let (code, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let upd = &v["hookSpecificOutput"]["updatedToolOutput"];
    assert!(upd["stdout"]
        .as_str()
        .unwrap()
        .starts_with("[dtk] jsonl: 300 rows x 4 cols"));
    assert_eq!(upd["stderr"], "warn: x");
    assert!(
        home.join(".local/share/dtk/store")
            .read_dir()
            .unwrap()
            .count()
            >= 1
    );
    let log = std::fs::read_to_string(home.join(".local/share/dtk/decisions.jsonl")).unwrap();
    assert!(log.contains("\"applied\":true"));
}

#[test]
fn leaves_small_output_alone() {
    let home = temp_home("small");
    let resp = json!({"stdout": jsonl(15), "stderr": "", "interrupted": false, "isImage": false});
    let (code, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    assert_eq!(code, 0);
    assert_eq!(out, "");
}

#[test]
fn leaves_non_table_output_alone() {
    let home = temp_home("prose");
    let prose = "Compiling foo v0.1.0\nwarning: unused variable `x`\n".repeat(400);
    let resp = json!({"stdout": prose, "stderr": "", "interrupted": false, "isImage": false});
    let (code, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn ignores_other_tools() {
    let home = temp_home("tool");
    let resp = json!({"stdout": jsonl(300)});
    let (code, out, _) = run(&home, &["hook", "claude"], &event("Read", resp), &[]);
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn survives_garbage_input() {
    let home = temp_home("garbage");
    for input in ["", "not json", "{\"hook_event_name\":1}", "[1,2,3]"] {
        let (code, out, _) = run(&home, &["hook", "claude"], input, &[]);
        assert_eq!((code, out.as_str()), (0, ""), "input {input:?}");
    }
}

#[test]
fn reads_the_saved_file_for_large_outputs() {
    let home = temp_home("persisted");
    let saved = home.join(".claude/tool-results/abc");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    std::fs::write(&saved, jsonl(2000)).unwrap();
    let head: String = jsonl(2000).chars().take(30000).collect();
    let resp = json!({
        "stdout": head, "stderr": "", "interrupted": false, "isImage": false,
        "persistedOutputPath": saved.display().to_string(), "persistedOutputSize": 99999
    });
    let (_, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    let v: Value = serde_json::from_str(&out).unwrap();
    let upd = &v["hookSpecificOutput"]["updatedToolOutput"];
    let text = upd["stdout"].as_str().unwrap();
    assert!(text.starts_with("[dtk] jsonl: 2000 rows"), "{text}");
    assert!(upd.get("persistedOutputPath").is_none());
    assert!(text.contains(&saved.display().to_string()));
}

#[test]
fn never_reads_files_outside_the_claude_folder() {
    let home = temp_home("outside");
    let other = home.join("secret.jsonl");
    std::fs::write(&other, jsonl(2000)).unwrap();
    let resp = json!({"stdout": "x", "persistedOutputPath": other.display().to_string()});
    let (code, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn disabled_and_shadow_change_nothing() {
    let home = temp_home("modes");
    let resp = json!({"stdout": jsonl(300), "stderr": "", "interrupted": false, "isImage": false});
    let ev = event("Bash", resp);
    let (_, out, _) = run(&home, &["hook", "claude"], &ev, &[("DTK_DISABLED", "1")]);
    assert_eq!(out, "");
    let (_, out, _) = run(&home, &["hook", "claude"], &ev, &[("DTK_MODE", "shadow")]);
    assert_eq!(out, "");
    let log = std::fs::read_to_string(home.join(".local/share/dtk/decisions.jsonl")).unwrap();
    assert!(
        log.contains("\"mode\":\"shadow\"")
            && log.contains("\"would_apply\":true")
            && log.contains("\"applied\":false")
    );
    assert!(
        !home.join(".local/share/dtk/store").exists(),
        "shadow mode must not write data files"
    );
}

#[test]
fn mode_command_persists() {
    let home = temp_home("modefile");
    assert_eq!(run(&home, &["mode", "shadow"], "", &[]).0, 0);
    assert_eq!(run(&home, &["mode"], "", &[]).1.trim(), "shadow");
    let resp = json!({"stdout": jsonl(300)});
    let (_, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    assert_eq!(out, "");
    assert_eq!(run(&home, &["mode", "live"], "", &[]).0, 0);
    assert_eq!(run(&home, &["mode"], "", &[]).1.trim(), "live");
}

#[test]
fn init_registers_and_removes_without_touching_other_hooks() {
    let home = temp_home("init");
    let settings = home.join(".claude/settings.json");
    let original = json!({
        "model": "opus",
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}],
            "PostToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "other-tool"}]}]
        }
    });
    std::fs::write(&settings, serde_json::to_string_pretty(&original).unwrap()).unwrap();
    assert_eq!(run(&home, &["init", "--global"], "", &[]).0, 0);
    assert_eq!(run(&home, &["init", "--global"], "", &[]).0, 0); // idempotent
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let post = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2, "other hook kept, one dtk hook added once");
    assert_eq!(v["model"], "opus");
    assert_eq!(v["hooks"]["PreToolUse"], original["hooks"]["PreToolUse"]);
    assert!(post[1]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("dtk hook claude"));
    assert!(home.join(".claude/settings.json.dtk-backup").exists());
    assert_eq!(run(&home, &["init", "--uninstall"], "", &[]).0, 0);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v, original);
}

#[test]
fn init_refuses_invalid_settings() {
    let home = temp_home("badjson");
    std::fs::write(home.join(".claude/settings.json"), "{ not json").unwrap();
    let (code, _, err) = run(&home, &["init", "--global"], "", &[]);
    assert_ne!(code, 0);
    assert!(err.contains("not valid JSON"));
    assert_eq!(
        std::fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        "{ not json"
    );
}

#[test]
fn a_closed_pipe_is_not_a_panic() {
    let home = temp_home("pipe");
    let mut child = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .arg("doctor")
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // the reader goes away before dtk prints
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
}
