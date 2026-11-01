//! `dtk init --global`: registers the hook in `~/.claude/settings.json`, and removes it again.
//!
//! The file is edited as JSON, key order is kept, unrelated hooks are left alone, and a backup
//! is written once.
//!
//! rtk integration: if rtk has its own `PostToolUse` hook, dtk takes over that registration and
//! runs rtk's hook itself, after dtk (`dtk hook claude --then rtk hook claude`). That gives one
//! hook and a fixed order. The folded entries are saved in `~/.config/dtk/init-state.json` so
//! `--uninstall` can restore them exactly.

use crate::hook::{config_dir, create_private_dir, home};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const DEFAULT_THEN: &str = "rtk hook claude";

pub fn settings_path() -> Option<PathBuf> {
    Some(home()?.join(".claude/settings.json"))
}

fn state_path() -> Option<PathBuf> {
    Some(config_dir()?.join("init-state.json"))
}

fn commands(entry: &Value) -> Vec<String> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hs| {
            hs.iter()
                .filter_map(|h| h.get("command").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// A hook entry that runs `dtk hook claude`.
pub fn is_dtk(entry: &Value) -> bool {
    commands(entry)
        .iter()
        .any(|c| c.contains("dtk") && c.contains("hook claude"))
}

/// A hook entry that holds only rtk hook commands.
pub fn is_only_rtk(entry: &Value) -> bool {
    let cmds = commands(entry);
    !is_dtk(entry) && !cmds.is_empty() && cmds.iter().all(|c| c.contains("rtk hook"))
}

pub fn rtk_on_path() -> bool {
    Command::new("rtk")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Union of matchers, always including `Bash`. `*` or an empty matcher means every tool.
fn union_matchers(parts: &[String]) -> String {
    let mut out: Vec<String> = vec!["Bash".to_string()];
    for m in parts {
        if m.is_empty() || m == "*" {
            return "*".to_string();
        }
        for p in m.split('|') {
            let p = p.trim();
            if !p.is_empty() && !out.iter().any(|o| o == p) {
                out.push(p.to_string());
            }
        }
    }
    out.join("|")
}

/// Entries that were folded into dtk's chain: (matcher, command).
fn load_folded() -> Vec<(String, String)> {
    let Some(text) = state_path().and_then(|p| fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v.get("folded")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some((
                        e.get("matcher")?.as_str()?.to_string(),
                        e.get("command")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_folded(folded: &[(String, String)]) {
    let Some(path) = state_path() else { return };
    if folded.is_empty() {
        let _ = fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = create_private_dir(dir);
    }
    let v = json!({"folded": folded.iter().map(|(m, c)| json!({"matcher": m, "command": c})).collect::<Vec<_>>()});
    let _ = fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
}

pub fn run(args: &[String]) -> i32 {
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let no_rtk = args.iter().any(|a| a == "--no-rtk");
    if !args.iter().any(|a| a == "--global") && !uninstall {
        eprintln!("usage: dtk init --global [--no-rtk] | dtk init --uninstall");
        return 2;
    }
    let Some(path) = settings_path() else {
        return 1;
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p.display().to_string(),
        Err(e) => {
            eprintln!("dtk: cannot find my own path: {e}");
            return 1;
        }
    };
    let mut settings: Value = match fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "dtk: {} is not valid JSON ({e}); not touching it",
                    path.display()
                );
                return 1;
            }
        },
        Err(_) => json!({}),
    };
    let Some(obj) = settings.as_object_mut() else {
        eprintln!(
            "dtk: {} is not a JSON object; not touching it",
            path.display()
        );
        return 1;
    };
    let backup = path.with_extension("json.dtk-backup");
    if path.exists() && !backup.exists() {
        if let Err(e) = fs::copy(&path, &backup) {
            eprintln!("dtk: cannot write the backup: {e}");
            return 1;
        }
    }
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        eprintln!("dtk: \"hooks\" in settings.json is not an object; not touching it");
        return 1;
    };
    let post = hooks.entry("PostToolUse").or_insert_with(|| json!([]));
    let Some(post) = post.as_array_mut() else {
        eprintln!("dtk: \"hooks.PostToolUse\" is not an array; not touching it");
        return 1;
    };

    let mut folded = load_folded();
    post.retain(|e| !is_dtk(e));
    let mut notes: Vec<String> = Vec::new();

    if uninstall || no_rtk {
        // Put back what an earlier install folded in.
        for (matcher, command) in folded.drain(..) {
            post.push(
                json!({"matcher": matcher, "hooks": [{"type": "command", "command": command}]}),
            );
            notes.push(format!(
                "restored the rtk PostToolUse hook (matcher {matcher})"
            ));
        }
    }
    if !uninstall {
        let chain = !no_rtk;
        let mut then_cmd = folded.first().map(|f| f.1.clone());
        if chain {
            // Fold rtk's own PostToolUse entries into the chain.
            let mut keep = Vec::new();
            for e in post.drain(..) {
                let m = e
                    .get("matcher")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let cmd = commands(&e).first().cloned();
                if is_only_rtk(&e) && (then_cmd.is_none() || then_cmd == cmd) {
                    then_cmd = cmd.clone();
                    notes.push(format!(
                        "folded the rtk PostToolUse hook (matcher {m}) into dtk's chain"
                    ));
                    folded.push((m, cmd.unwrap_or_default()));
                } else {
                    keep.push(e);
                }
            }
            *post = keep;
        }
        let use_chain = chain && (then_cmd.is_some() || rtk_on_path());
        let matchers: Vec<String> = folded.iter().map(|f| f.0.clone()).collect();
        let matcher = if use_chain {
            union_matchers(&matchers)
        } else {
            "Bash".to_string()
        };
        let mut command = format!("{exe} hook claude");
        if use_chain {
            command.push_str(" --then ");
            command.push_str(then_cmd.as_deref().unwrap_or(DEFAULT_THEN));
            notes.push(
                "chained to rtk: dtk handles tables first, rtk gets every other event".to_string(),
            );
        }
        post.push(json!({"matcher": matcher, "hooks": [{"type": "command", "command": command}]}));
    }
    if post.is_empty() {
        hooks.remove("PostToolUse");
    }
    // Leave no empty "hooks" object behind when dtk created it.
    if hooks.is_empty() {
        obj.remove("hooks");
    }
    save_folded(&folded);
    let mut text = serde_json::to_string_pretty(&settings).unwrap_or_default();
    text.push('\n');
    if let Err(e) = fs::write(&path, text) {
        eprintln!("dtk: cannot write {}: {e}", path.display());
        return 1;
    }
    if uninstall {
        println!("dtk: hook removed from {}", path.display());
    } else {
        println!(
            "dtk: hook registered in {} (backup: {})",
            path.display(),
            backup.display()
        );
        println!("dtk: restart Claude Code (or start a new session) to load it");
    }
    for n in notes {
        println!("dtk: {n}");
    }
    0
}
