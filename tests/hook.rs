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
    // The path in the summary leads to the saved file (directly, or through a short link).
    let raw_line = text.lines().find(|l| l.starts_with("raw: ")).expect(text);
    let shown = raw_line
        .trim_start_matches("raw: ")
        .split(" (")
        .next()
        .unwrap()
        .trim_matches('\'');
    assert_eq!(
        std::fs::canonicalize(shown).unwrap(),
        std::fs::canonicalize(&saved).unwrap(),
        "{raw_line}"
    );
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
        .ends_with(" hook claude"));
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

fn read_event(path: &str, content: &str, extra_input: Value) -> String {
    let lines = content.lines().count();
    let mut input = json!({"file_path": path});
    if let (Some(o), Some(e)) = (input.as_object_mut(), extra_input.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": input,
        "tool_response": {"type": "text", "file": {
            "filePath": path, "content": content,
            "numLines": lines, "startLine": 1, "totalLines": lines
        }},
    })
    .to_string()
}

#[test]
fn a_whole_data_file_read_with_the_read_tool_is_summarized() {
    let home = temp_home("read");
    let content = jsonl(300);
    let ev = read_event("/work/data/events.jsonl", &content, json!({}));
    let (code, out, _) = run(&home, &["hook", "claude"], &ev, &[]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let file = &v["hookSpecificOutput"]["updatedToolOutput"]["file"];
    let text = file["content"].as_str().unwrap();
    assert!(text.starts_with("[dtk] jsonl: 300 rows x 4 cols"), "{text}");
    assert!(
        text.contains("raw: /work/data/events.jsonl"),
        "the hint points at the real file: {text}"
    );
    assert_eq!(file["filePath"], "/work/data/events.jsonl");
    assert_eq!(file["numLines"], text.lines().count());
    assert_eq!(v["hookSpecificOutput"]["updatedToolOutput"]["type"], "text");
}

#[test]
fn reads_with_a_range_other_files_and_small_files_are_left_alone() {
    let home = temp_home("readskip");
    let big = jsonl(300);
    let run_ev = |ev: String| run(&home, &["hook", "claude"], &ev, &[]).1;
    assert_eq!(
        run_ev(read_event("/w/e.jsonl", &big, json!({"offset": 10}))),
        "",
        "offset means exact lines"
    );
    assert_eq!(
        run_ev(read_event("/w/e.jsonl", &big, json!({"limit": 50}))),
        "",
        "limit means exact lines"
    );
    assert_eq!(
        run_ev(read_event("/w/main.rs", &big, json!({}))),
        "",
        "source files are never checked"
    );
    assert_eq!(
        run_ev(read_event("/w/e.jsonl", &jsonl(15), json!({}))),
        "",
        "small files pass"
    );
    // A partial read: the tool returned fewer lines than the file has.
    let mut partial: Value =
        serde_json::from_str(&read_event("/w/e.jsonl", &big, json!({}))).unwrap();
    partial["tool_response"]["file"]["totalLines"] = json!(9999);
    assert_eq!(run_ev(partial.to_string()), "");
}

#[test]
fn commands_that_ask_for_rows_get_rows() {
    let home = temp_home("raw");
    let resp = json!({"stdout": jsonl(300), "stderr": "", "interrupted": false, "isImage": false});
    for cmd in [
        "qsv slice -s 100 -l 200 /tmp/x.csv",
        "DTK_RAW=1 cat data.jsonl",
        "cat data.jsonl | head -300",
        "tail -n 300 app.log",
        "sed -n '1,300p' data.jsonl",
        "rtk read app.log --tail-lines 300",
    ] {
        let ev = json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": cmd}, "tool_response": resp,
        })
        .to_string();
        let (code, out, _) = run(&home, &["hook", "claude"], &ev, &[]);
        assert_eq!((code, out.as_str()), (0, ""), "{cmd}");
    }
    // The same output from a plain cat is summarized.
    let ev = json!({
        "hook_event_name": "PostToolUse", "tool_name": "Bash",
        "tool_input": {"command": "cat data.jsonl"}, "tool_response": resp,
    })
    .to_string();
    assert!(run(&home, &["hook", "claude"], &ev, &[])
        .1
        .contains("[dtk] jsonl"));
}

#[test]
fn a_polars_table_from_a_python_command_is_summarized() {
    let home = temp_home("polars");
    let mut s = String::from("shape: (400, 3)\n┌──────┬───────┬────────┐\n│ id   ┆ name  ┆ amount │\n│ ---  ┆ ---   ┆ ---    │\n│ i64  ┆ str   ┆ f64    │\n╞══════╪═══════╪════════╡\n");
    for i in 0..400 {
        s.push_str(&format!(
            "│ {:<4} ┆ user{} ┆ {:<6} │\n",
            i,
            i % 5,
            i as f64 * 1.5
        ));
    }
    s.push_str("└──────┴───────┴────────┘\n");
    let resp = json!({"stdout": s, "stderr": "", "interrupted": false, "isImage": false});
    let ev = json!({
        "hook_event_name": "PostToolUse", "tool_name": "Bash",
        "tool_input": {"command": "python3 report.py"}, "tool_response": resp,
    })
    .to_string();
    let (_, out, _) = run(&home, &["hook", "claude"], &ev, &[]);
    let v: Value = serde_json::from_str(&out).unwrap();
    let text = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap();
    assert!(text.starts_with("[dtk] table: 400 rows x 3 cols"), "{text}");
    assert!(text.contains("note: shape: (400, 3)"), "{text}");
    assert!(
        text.contains("cols: id:int name:str amount:float"),
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn a_long_saved_path_gets_a_short_link_in_the_store() {
    let home = temp_home("shortlink");
    let deep = home.join(".claude/projects/-Users-someone-very-long-project-name-here/0123456789abcdef0123456789abcdef/tool-results");
    std::fs::create_dir_all(&deep).unwrap();
    let saved = deep.join("bxyz123");
    std::fs::write(&saved, jsonl(2000)).unwrap();
    assert!(saved.display().to_string().len() > 60);
    let head: String = jsonl(2000).chars().take(30000).collect();
    let resp = json!({"stdout": head, "stderr": "", "interrupted": false, "isImage": false,
        "persistedOutputPath": saved.display().to_string(), "persistedOutputSize": 99999});
    let (_, out, _) = run(&home, &["hook", "claude"], &event("Bash", resp), &[]);
    let v: Value = serde_json::from_str(&out).unwrap();
    let text = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap();
    assert!(
        !text.contains(&saved.display().to_string()),
        "the long path must not appear: {text}"
    );
    let raw_line = text.lines().find(|l| l.starts_with("raw: ")).unwrap();
    let link = raw_line
        .trim_start_matches("raw: ")
        .split(' ')
        .next()
        .unwrap();
    assert!(
        link.contains("/.local/share/dtk/store/") && link.ends_with(".jsonl"),
        "{raw_line}"
    );
    assert_eq!(
        std::fs::read_link(link).unwrap(),
        saved,
        "the link points at the real file"
    );
    assert_eq!(
        std::fs::read_to_string(link).unwrap(),
        jsonl(2000),
        "and reads through"
    );
}

/// `dtk init` leaves a copy of dtk named `rtk`. As `rtk hook claude` it must do nothing, silently.
#[test]
fn the_placeholder_does_nothing_for_hook_claude_and_says_so_otherwise() {
    let home = temp_home("placeholder");
    // A fake rtk on PATH makes init chain, which is when it creates the placeholder.
    let bin = home.join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    let rtk_name = if cfg!(windows) { "rtk.cmd" } else { "rtk" };
    std::fs::write(
        bin.join(rtk_name),
        if cfg!(windows) {
            "@echo off\r\n"
        } else {
            "#!/bin/sh\nexit 0\n"
        },
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join(rtk_name), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    let path = format!(
        "{}{}{}",
        bin.display(),
        if cfg!(windows) { ";" } else { ":" },
        std::env::var("PATH").unwrap_or_default()
    );
    let (code, _, _) = run(&home, &["init", "--global"], "", &[("PATH", &path)]);
    assert_eq!(code, 0);
    let shim =
        home.join(".local/share/dtk/shim")
            .join(if cfg!(windows) { "rtk.exe" } else { "rtk" });
    assert!(shim.exists(), "the placeholder was not created");
    let go = |args: &[&str]| {
        let mut child = Command::new(&shim)
            .args(args)
            .env("HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"hook_event_name\":\"PostToolUse\"}");
        let o = child.wait_with_output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )
    };
    assert_eq!(
        go(&["hook", "claude"]),
        (0, String::new(), String::new()),
        "silent success"
    );
    let (code, out, err) = go(&["gain"]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("placeholder for rtk"), "{err}");
}
