//! dtk and the REAL rtk binary: what rtk's PreToolUse hook does to a command, and what rtk does
//! to output, must not make dtk do the wrong thing.
//!
//! - rtk rewrites `head -n 400 f` to `rtk read f --head-lines 400`. The user asked for 400 rows,
//!   so dtk must not turn them into a summary.
//! - rtk rewrites `cat f` to `rtk read f`. An `exclude_commands = ["^cat "]` rule must still work.
//! - rtk may cut a long output and add a pointer to the rest. A summary of the part that is left
//!   would claim to cover all rows and would hide that pointer.
//!
//! Skipped (with a message) when `rtk` is not on PATH. CI installs rtk.

#![cfg(not(windows))]

use std::fs;
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

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-compat-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".config/dtk")).unwrap();
    dir.canonicalize().unwrap()
}

fn with_home(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("USERPROFILE", home)
        .stdin(Stdio::null());
}

fn text(o: std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// What rtk's PreToolUse hook turns `cmd` into.
fn rtk_rewrite(home: &Path, cmd: &str) -> String {
    let mut c = Command::new("rtk");
    c.args(["rewrite", cmd]);
    with_home(&mut c, home);
    let out = String::from_utf8_lossy(&c.output().unwrap().stdout)
        .trim()
        .to_string();
    // No output means rtk leaves the command as it is.
    if out.is_empty() {
        cmd.to_string()
    } else {
        out
    }
}

fn explain(home: &Path, file: &Path, command: &str) -> String {
    let mut c = Command::new(env!("CARGO_BIN_EXE_dtk"));
    c.arg("explain").arg(file).args(["--command", command]);
    with_home(&mut c, home);
    text(c.output().unwrap())
}

fn big_csv(dir: &Path) -> PathBuf {
    let mut s = String::from("id,name,region,amount,status\n");
    for i in 0..3000 {
        s.push_str(&format!(
            "{i},user_{},{},{}.25,{}\n",
            i % 97,
            ["us", "eu", "ap"][i % 3],
            i * 7 % 1000,
            ["ok", "ok", "ok", "failed"][i % 4]
        ));
    }
    let p = dir.join("big.csv");
    fs::write(&p, s).unwrap();
    p
}

#[test]
fn rows_the_user_asked_for_stay_rows_after_rtk_rewrites_the_command() {
    need_rtk!();
    let dir = workdir("rows");
    let file = big_csv(&dir);
    let f = file.display();
    for typed in [
        format!("head -n 400 {f}"),
        format!("tail -n 400 {f}"),
        format!("sed -n '1,400p' {f}"),
    ] {
        let ran = rtk_rewrite(&dir, &typed);
        // The rows that command prints, fed to dtk with the command that rtk really ran.
        let out = explain(&dir, &file, &ran);
        assert!(
            out.contains("PASS THROUGH") && out.contains("exact rows"),
            "`{typed}` ran as `{ran}`, and dtk would summarize the rows:\n{out}"
        );
    }
}

#[test]
fn a_plain_cat_is_still_summarized_after_the_rewrite() {
    need_rtk!();
    let dir = workdir("cat");
    let file = big_csv(&dir);
    let ran = rtk_rewrite(&dir, &format!("cat {}", file.display()));
    let out = explain(&dir, &file, &ran);
    assert!(out.contains("decision:  APPLY"), "`{ran}`:\n{out}");
}

#[test]
fn exclude_commands_still_match_what_the_user_typed() {
    need_rtk!();
    let dir = workdir("exclude");
    let file = big_csv(&dir);
    fs::write(
        dir.join(".config/dtk/config.toml"),
        "[hooks]\nexclude_commands = [\"^cat \", \"^curl\", \"jq \"]\n",
    )
    .unwrap();
    for typed in [
        format!("cat {}", file.display()),
        format!("curl -s file://{}", file.display()),
        format!("jq . {}", file.display()),
    ] {
        let ran = rtk_rewrite(&dir, &typed);
        let out = explain(&dir, &file, &ran);
        assert!(
            out.contains("exclude_commands"),
            "`{typed}` ran as `{ran}`, and the exclusion no longer matches:\n{out}"
        );
    }
}

#[test]
fn output_that_rtk_cut_is_left_alone() {
    need_rtk!();
    let dir = workdir("cut");
    let json = dir.join("big.json");
    let items: Vec<String> = (0..2000)
        .map(|i| format!("{{\"id\":{i},\"name\":\"n{i}\",\"score\":{}}}", i * 3))
        .collect();
    fs::write(&json, format!("[{}]", items.join(","))).unwrap();
    // rtk's jq filter cuts a long document and adds "... (N lines truncated)" and a pointer.
    let mut c = Command::new("rtk");
    c.args(["jq", "."]).arg(&json);
    with_home(&mut c, &dir);
    let cut = text(c.output().unwrap());
    assert!(
        cut.contains("lines truncated)"),
        "rtk no longer cuts this way; update the test:\n{cut}"
    );
    let shown = dir.join("cut.txt");
    fs::write(&shown, &cut).unwrap();
    let out = explain(&dir, &shown, "rtk jq . big.json");
    assert!(
        !out.contains("decision:  APPLY"),
        "dtk summarized a document that rtk had cut:\n{out}"
    );
}

/// rtk's post-command pipeline, which cuts a long output to a few hundred lines.
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

/// Runs `dtk hook claude --then rtk hook claude` on a Bash result and returns what it printed.
fn chain(home: &Path, command: &str, stdout: &str) -> String {
    use std::io::Write;
    let event = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
        "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false, "isImage": false},
    })
    .to_string();
    let mut c = Command::new(env!("CARGO_BIN_EXE_dtk"));
    c.args(["hook", "claude", "--then", "rtk", "hook", "claude"]);
    with_home(&mut c, home);
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.as_bytes())
        .unwrap();
    String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).to_string()
}

#[test]
fn rtks_pipeline_never_touches_the_output_of_a_dtk_command() {
    need_rtk!();
    let home = workdir("pipeline");
    enable_posthook(&home);
    // 850 plain lines: rtk's pipeline cuts these, which proves that the pipeline is on.
    let long: String = (0..850)
        .map(|i| format!("line {i} of a long plain text output\n"))
        .collect();
    let cut = chain(&home, "mytool --dump", &long);
    assert!(
        cut.contains("updatedToolOutput"),
        "the pipeline is not on, so the test proves nothing:\n{cut}"
    );
    // The same output from dtk's own commands is left exactly as it was.
    for cmd in [
        "dtk where abcdef012345 region = us 900",
        "/usr/local/bin/dtk rows abcdef012345 0 900",
        "cd d && dtk group abcdef012345 region amount",
        "rtk dtk top abcdef012345 amount 900",
    ] {
        let out = chain(&home, cmd, &long);
        assert!(
            out.trim().is_empty(),
            "`{cmd}` was changed:\n{}",
            &out[..out.len().min(300)]
        );
    }
}
