//! How dtk works with rtk: the hook chain, folding rtk's hook in, restoring it, doctor and gain.
//! A fake `rtk` shell script stands in for the real one, with a throwaway HOME.

#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-rtk-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
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

/// A directory with an executable `rtk` script. Returns the directory.
fn fake_rtk(home: &Path, body: &str) -> PathBuf {
    let bin = home.join("fakebin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("rtk");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// Answers `gain`, `--version`, and any hook event (the event goes to $FAKE_LOG).
const RTK_ANSWERS: &str = r#"case "$1" in
  gain) printf '%s' '{"summary":{"total_saved":123,"total_commands":9}}' ;;
  --version) echo "rtk 0.0.0" ;;
  *) cat > "$FAKE_LOG"; printf '%s' '{"from":"rtk"}' ;;
esac"#;

fn run(
    home: &Path,
    bin: Option<&Path>,
    args: &[&str],
    stdin: &str,
    env: &[(&str, &str)],
) -> (i32, String, String) {
    let path = match bin {
        Some(b) => format!("{}:/usr/bin:/bin", b.display()),
        None => "/usr/bin:/bin".to_string(),
    };
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dtk"));
    cmd.args(args)
        .env("HOME", home)
        .env("PATH", path)
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

const THEN: [&str; 6] = ["hook", "claude", "--then", "rtk", "hook", "claude"];

#[test]
fn a_table_is_handled_by_dtk_and_rtk_is_never_called() {
    let home = temp_home("table");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    let log = home.join("rtk-saw.json");
    let resp = json!({"stdout": jsonl(300), "stderr": "", "interrupted": false, "isImage": false});
    let (code, out, _) = run(
        &home,
        Some(&bin),
        &THEN,
        &event("Bash", resp),
        &[("FAKE_LOG", log.to_str().unwrap())],
    );
    assert_eq!(code, 0);
    assert!(out.contains("[dtk] jsonl: 300 rows"), "{out}");
    assert!(!log.exists(), "rtk must not see an event that dtk handled");
}

#[test]
fn every_other_event_reaches_rtk_byte_for_byte() {
    let home = temp_home("other");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    let log = home.join("rtk-saw.json");
    let envs = [("FAKE_LOG", log.to_str().unwrap())];
    // Another tool.
    let read = event("Read", json!({"stdout": "hello"}));
    let (code, out, _) = run(&home, Some(&bin), &THEN, &read, &envs);
    assert_eq!((code, out.as_str()), (0, "{\"from\":\"rtk\"}"));
    assert_eq!(fs::read_to_string(&log).unwrap(), read);
    // Bash output that is not a table.
    fs::remove_file(&log).unwrap();
    let prose = event(
        "Bash",
        json!({"stdout": "Compiling foo v0.1.0\n".repeat(400), "stderr": ""}),
    );
    let (_, out, _) = run(&home, Some(&bin), &THEN, &prose, &envs);
    assert_eq!(out, "{\"from\":\"rtk\"}");
    assert_eq!(fs::read_to_string(&log).unwrap(), prose);
}

#[test]
fn rtk_exit_code_and_stderr_pass_through() {
    let home = temp_home("exit");
    let bin = fake_rtk(&home, "cat >/dev/null; echo 'rtk says no' >&2; exit 2");
    let (code, _, err) = run(
        &home,
        Some(&bin),
        &THEN,
        &event("Read", json!({"stdout": "x"})),
        &[],
    );
    assert_eq!(code, 2);
    assert!(err.contains("rtk says no"), "{err}");
}

#[test]
fn a_missing_or_slow_chain_command_fails_open() {
    let home = temp_home("slow");
    let ev = event("Read", json!({"stdout": "x"}));
    let (code, out, _) = run(
        &home,
        None,
        &["hook", "claude", "--then", "no-such-binary-xyz"],
        &ev,
        &[],
    );
    assert_eq!((code, out.as_str()), (0, ""));
    // `exec` makes the sleeper the chained process itself, so the kill reaches it.
    let bin = fake_rtk(&home, "exec sleep 5");
    let start = Instant::now();
    let (code, out, _) = run(
        &home,
        Some(&bin),
        &THEN,
        &ev,
        &[("DTK_CHAIN_TIMEOUT_MS", "200")],
    );
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(start.elapsed().as_secs() < 4, "the chain must be cut off");
}

#[test]
fn a_chain_that_leaves_a_child_holding_the_pipe_does_not_hang_dtk() {
    let home = temp_home("orphan");
    // The answer is printed and the script exits, but a background sleeper keeps stdout open.
    let bin = fake_rtk(
        &home,
        "cat >/dev/null; sleep 3 2>/dev/null &\nprintf '%s' '{\"from\":\"rtk\"}'",
    );
    let ev = event("Read", json!({"stdout": "x"}));
    let start = Instant::now();
    let (code, out, _) = run(&home, Some(&bin), &THEN, &ev, &[]);
    assert_eq!((code, out.as_str()), (0, "{\"from\":\"rtk\"}"));
    assert!(
        start.elapsed().as_millis() < 2500,
        "dtk waited for the orphan: {:?}",
        start.elapsed()
    );
}

#[test]
fn disabled_dtk_still_runs_the_chain() {
    let home = temp_home("disabled");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    let log = home.join("rtk-saw.json");
    let ev = event("Bash", json!({"stdout": jsonl(300), "stderr": ""}));
    let (_, out, _) = run(
        &home,
        Some(&bin),
        &THEN,
        &ev,
        &[("FAKE_LOG", log.to_str().unwrap()), ("DTK_DISABLED", "1")],
    );
    assert_eq!(out, "{\"from\":\"rtk\"}");
    assert_eq!(fs::read_to_string(&log).unwrap(), ev);
}

fn read_settings(home: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap()).unwrap()
}

#[test]
fn init_folds_rtks_post_hook_into_the_chain_and_uninstall_restores_it() {
    let home = temp_home("fold");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    let original = json!({
        "model": "opus",
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}],
            "PostToolUse": [
                {"matcher": "*", "hooks": [{"type": "command", "command": "other-tool"}]},
                {"matcher": "Bash|Grep|WebFetch", "hooks": [{"type": "command", "command": "rtk hook claude"}]}
            ]
        }
    });
    fs::write(
        home.join(".claude/settings.json"),
        serde_json::to_string_pretty(&original).unwrap(),
    )
    .unwrap();
    let (code, out, _) = run(&home, Some(&bin), &["init", "--global"], "", &[]);
    assert_eq!(code, 0);
    assert!(out.contains("took over the rtk PostToolUse hook"), "{out}");
    run(&home, Some(&bin), &["init", "--global"], "", &[]); // idempotent
    let s = read_settings(&home);
    let post = s["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(
        post.len(),
        2,
        "other hook kept, rtk's entry folded, one dtk entry: {post:?}"
    );
    assert_eq!(post[0]["hooks"][0]["command"], "other-tool");
    assert_eq!(post[1]["matcher"], "Bash|Read|mcp__.*|Grep|WebFetch");
    assert!(post[1]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("dtk hook claude --then rtk hook claude"));
    assert_eq!(
        s["hooks"]["PreToolUse"], original["hooks"]["PreToolUse"],
        "rtk's PreToolUse hook is untouched"
    );
    let (_, out, _) = run(&home, Some(&bin), &["init", "--uninstall"], "", &[]);
    assert!(out.contains("restored the rtk PostToolUse hook"), "{out}");
    assert_eq!(read_settings(&home), original);
}

#[test]
fn init_chains_to_rtk_when_rtk_has_no_post_hook_yet() {
    let home = temp_home("chainonly");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    run(&home, Some(&bin), &["init", "--global"], "", &[]);
    let s = read_settings(&home);
    let e = &s["hooks"]["PostToolUse"][0];
    assert_eq!(e["matcher"], "Bash|Read|mcp__.*");
    assert!(e["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("hook claude --then rtk hook claude"));
    run(&home, Some(&bin), &["init", "--uninstall"], "", &[]);
    assert!(read_settings(&home).get("hooks").is_none());
}

#[test]
fn init_without_rtk_or_with_no_rtk_does_not_chain() {
    let home = temp_home("nochain");
    run(&home, None, &["init", "--global"], "", &[]);
    let cmd = read_settings(&home)["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(cmd.ends_with("dtk hook claude"), "{cmd}");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    run(
        &home,
        Some(&bin),
        &["init", "--global", "--no-rtk"],
        "",
        &[],
    );
    let cmd = read_settings(&home)["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(cmd.ends_with("dtk hook claude"), "{cmd}");
}

#[test]
fn doctor_reports_the_chain_and_flags_a_parallel_rtk_hook() {
    let home = temp_home("doctor");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    let rtk_pre =
        json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]});
    let settings = json!({"hooks": {"PreToolUse": [rtk_pre]}});
    fs::write(home.join(".claude/settings.json"), settings.to_string()).unwrap();
    run(&home, Some(&bin), &["init", "--global"], "", &[]);
    let (_, out, _) = run(&home, Some(&bin), &["doctor"], "", &[]);
    assert!(out.contains("[ok] rtk found on PATH"), "{out}");
    assert!(
        out.contains("[ok] rtk PreToolUse hook is registered"),
        "{out}"
    );
    assert!(out.contains("[ok] dtk is chained to rtk"), "{out}");
    // Now rtk adds its own PostToolUse entry again, as `rtk init -g` would.
    let mut s = read_settings(&home);
    s["hooks"]["PostToolUse"].as_array_mut().unwrap().push(
        json!({"matcher": "Grep", "hooks": [{"type": "command", "command": "rtk hook claude"}]}),
    );
    fs::write(home.join(".claude/settings.json"), s.to_string()).unwrap();
    let (_, out, _) = run(&home, Some(&bin), &["doctor"], "", &[]);
    assert!(
        out.contains("[warn] a separate rtk PostToolUse hook runs next to dtk"),
        "{out}"
    );
}

#[test]
fn gain_shows_rtk_totals_beside_dtk() {
    let home = temp_home("gain");
    let bin = fake_rtk(&home, RTK_ANSWERS);
    let (code, out, _) = run(&home, Some(&bin), &["gain"], "", &[]);
    assert_eq!(code, 0);
    assert!(
        out.contains("rtk (rtk gain): 123 tokens saved over 9 commands"),
        "{out}"
    );
    let (_, out, _) = run(&home, None, &["gain"], "", &[]);
    assert!(out.contains("rtk: not found"), "{out}");
}
