//! `dtk doctor`: checks that the hook, rtk and the helper tools are wired up.

use crate::hook::{config_dir, data_dir, is_live};
use crate::init::{is_dtk, is_only_rtk, rtk_on_path, settings_path};
use serde_json::Value;
use std::fs;
use std::process::{Command, Stdio};

fn on_path(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn line(tag: &str, text: &str) {
    println!("[{tag}] {text}");
}

pub fn run() -> i32 {
    line(
        "ok",
        &format!(
            "dtk {} at {}",
            env!("CARGO_PKG_VERSION"),
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ),
    );
    line(
        "ok",
        &format!(
            "mode: {}",
            if is_live() {
                "live"
            } else {
                "shadow (logs only, changes nothing)"
            }
        ),
    );

    let settings: Option<Value> = settings_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok());
    let post: Vec<Value> = settings
        .as_ref()
        .and_then(|s| s.get("hooks"))
        .and_then(|h| h.get("PostToolUse"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let dtk_entries: Vec<&Value> = post.iter().filter(|e| is_dtk(e)).collect();
    let mut chained = false;
    match dtk_entries.len() {
        0 => line(
            "warn",
            "dtk is not registered in ~/.claude/settings.json. Run: dtk init --global",
        ),
        1 => {
            let e = dtk_entries[0];
            let cmd = e
                .get("hooks")
                .and_then(Value::as_array)
                .and_then(|h| h.first())
                .and_then(|h| h.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("");
            chained = cmd.contains("--then");
            let exe = cmd.split_whitespace().next().unwrap_or("");
            let matcher = e.get("matcher").and_then(Value::as_str).unwrap_or("");
            if matcher != "*" && !matcher.split('|').any(|m| m == "Read") {
                line(
                    "warn",
                    "the hook was registered before Read support, so data files read with the Read tool are not summarized. Run: dtk init --global",
                );
            }
            if std::path::Path::new(exe).exists() {
                line(
                    "ok",
                    &format!("PostToolUse hook registered (matcher {matcher}): {cmd}"),
                );
            } else {
                line(
                    "warn",
                    &format!(
                        "registered hook points at a missing file: {exe}. Run: dtk init --global"
                    ),
                );
            }
        }
        n => line(
            "warn",
            &format!("dtk is registered {n} times. Run: dtk init --global"),
        ),
    }

    let rtk = rtk_on_path();
    let pre_rtk = settings
        .as_ref()
        .and_then(|s| s.get("hooks"))
        .and_then(|h| h.get("PreToolUse"))
        .and_then(Value::as_array)
        .map(|a| a.iter().any(is_only_rtk))
        .unwrap_or(false);
    if rtk {
        line("ok", "rtk found on PATH");
        if pre_rtk {
            line(
                "ok",
                "rtk PreToolUse hook is registered (rtk rewrites commands, dtk summarizes tables)",
            );
        } else {
            line("--", "no rtk PreToolUse hook found. Run: rtk init -g");
        }
        let parallel = post.iter().filter(|e| is_only_rtk(e)).count();
        if parallel > 0 {
            line("warn", "a separate rtk PostToolUse hook runs next to dtk, so both may change one output. Run: dtk init --global");
        } else if chained {
            line(
                "ok",
                "dtk is chained to rtk: one PostToolUse hook, dtk first, then rtk",
            );
        } else if !dtk_entries.is_empty() {
            line("--", "dtk is not chained to rtk. Run: dtk init --global (needed only if you use rtk's PostToolUse pipeline)");
        }
    } else {
        line(
            "--",
            "rtk not found on PATH (optional; see https://github.com/rtk-ai/rtk)",
        );
        if chained {
            line(
                "warn",
                "dtk is chained to rtk, but rtk is not on PATH. Run: dtk init --global --no-rtk",
            );
        }
    }

    match data_dir().map(|d| d.join("store")) {
        Some(d) if d.exists() => line("ok", &format!("data store: {}", d.display())),
        _ => line(
            "--",
            "data store not created yet (created on first summary)",
        ),
    }
    match config_dir() {
        Some(_) => {}
        None => line("warn", "HOME is not set"),
    }
    for (name, why) in [("qsv", "CSV queries"), ("jaq", "JSON queries")] {
        if on_path(name) {
            line("ok", &format!("{name} found ({why} in summaries)"));
        } else {
            line("warn", &format!("{name} not found. Summaries point at it for {why}. Install: brew install {name}"));
        }
    }
    0
}
