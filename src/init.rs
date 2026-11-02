//! `dtk init --global`: registers the hook in `~/.claude/settings.json`, and removes it again.
//!
//! The file is edited as JSON, key order is kept, unrelated hooks are left alone, and a backup
//! is written once.
//!
//! rtk integration. dtk registers ONE `PostToolUse` entry with two hooks:
//!
//! 1. `dtk hook claude --then rtk hook claude`: does the work. dtk handles tables, and every other
//!    event goes to rtk byte for byte.
//! 2. A silent placeholder called `rtk` in dtk's data folder. rtk recognizes its own hook by the
//!    shape `<path>/rtk hook claude`. With the placeholder in place, `rtk init` sees its hook as
//!    installed and adds no second one (a second hook would run in parallel, and Claude Code
//!    applies the last matching entry). `rtk init --uninstall` removes only the placeholder.
//!
//! If rtk already has its own `PostToolUse` entry, dtk takes it over. The original is saved in
//! `~/.config/dtk/init-state.json`, so `--uninstall` can give it back.

use crate::hook::{config_dir, create_private_dir, data_dir, home};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const DEFAULT_THEN: &str = "rtk hook claude";
/// Tools dtk handles itself: Bash output and whole-file reads of data files.
const OWN_MATCHER: &str = "Bash|Read";
const SHIM_BODY: &str = "#!/bin/sh\n# dtk placeholder. It lets `rtk init` see its PostToolUse hook as installed.\n# The real rtk runs inside dtk's hook chain. This script does nothing.\nexit 0\n";

pub fn settings_path() -> Option<PathBuf> {
    Some(home()?.join(".claude/settings.json"))
}

fn state_path() -> Option<PathBuf> {
    Some(config_dir()?.join("init-state.json"))
}

pub fn shim_path() -> Option<PathBuf> {
    Some(data_dir()?.join("shim").join("rtk"))
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

/// The silent placeholder command.
pub fn is_placeholder(cmd: &str) -> bool {
    let c = cmd.replace('"', "");
    c.contains("/dtk/shim/rtk") && c.trim_end().ends_with("hook claude")
}

/// A hook entry that holds dtk's chain, or its placeholder.
pub fn is_dtk(entry: &Value) -> bool {
    commands(entry)
        .iter()
        .any(|c| c.contains("dtk") && c.contains("hook claude"))
}

pub fn has_placeholder(entry: &Value) -> bool {
    commands(entry).iter().any(|c| is_placeholder(c))
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

/// Creates the placeholder script and returns its hook command.
fn ensure_shim() -> Option<String> {
    let p = shim_path()?;
    create_private_dir(p.parent()?).ok()?;
    if fs::read_to_string(&p).ok().as_deref() != Some(SHIM_BODY) {
        fs::write(&p, SHIM_BODY).ok()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).ok()?;
    }
    let s = p.display().to_string();
    Some(if s.contains(char::is_whitespace) || s.contains('"') {
        format!("\"{}\" hook claude", s.replace('"', "\\\""))
    } else {
        format!("{s} hook claude")
    })
}

/// Distinct tokens of the given matchers, in order. `*` or an empty matcher means every tool.
fn union_tokens(parts: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
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

fn entry(matcher: &str, commands: &[String]) -> Value {
    json!({
        "matcher": matcher,
        "hooks": commands.iter().map(|c| json!({"type": "command", "command": c})).collect::<Vec<_>>()
    })
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
    // What an earlier install left: its matchers (rtk may have widened them), and whether the
    // placeholder is still there. rtk's own uninstall removes the placeholder.
    let mut own_matchers: Vec<String> = Vec::new();
    let mut had_placeholder = false;
    for e in post.iter().filter(|e| is_dtk(e)) {
        if let Some(m) = e.get("matcher").and_then(Value::as_str) {
            own_matchers.push(m.to_string());
        }
        had_placeholder |= has_placeholder(e);
    }
    post.retain(|e| !is_dtk(e));
    let mut notes: Vec<String> = Vec::new();

    if uninstall || no_rtk {
        // rtk's registration lived inside dtk's entry. Give it back, unless rtk itself removed it.
        if had_placeholder {
            let extras = own_matchers
                .iter()
                .flat_map(|m| m.split('|'))
                .any(|t| !OWN_MATCHER.split('|').any(|o| o == t));
            let matchers: Vec<String> = if !folded.is_empty() {
                folded.iter().map(|f| f.0.clone()).collect()
            } else if extras {
                own_matchers.clone()
            } else {
                Vec::new()
            };
            if !matchers.is_empty() {
                let matcher = union_tokens(&matchers);
                let cmd = folded
                    .first()
                    .map(|f| f.1.clone())
                    .unwrap_or_else(|| DEFAULT_THEN.to_string());
                post.push(entry(&matcher, &[cmd]));
                notes.push(format!(
                    "restored the rtk PostToolUse hook (matcher {matcher})"
                ));
            }
        }
        folded.clear();
    }
    if !uninstall {
        let chain = !no_rtk;
        if chain {
            // Take over rtk's own PostToolUse entries.
            let mut keep = Vec::new();
            for e in post.drain(..) {
                if is_only_rtk(&e) {
                    let m = e
                        .get("matcher")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let cmd = commands(&e).first().cloned().unwrap_or_default();
                    notes.push(format!("took over the rtk PostToolUse hook (matcher {m})"));
                    if !folded.iter().any(|f| f.0 == m && f.1 == cmd) {
                        folded.push((m, cmd));
                    }
                } else {
                    keep.push(e);
                }
            }
            *post = keep;
        }
        let then_cmd = folded.first().map(|f| f.1.clone());
        let use_chain = chain
            && (then_cmd.is_some()
                || (!own_matchers.is_empty() && had_placeholder)
                || rtk_on_path());
        let mut command = format!("{exe} hook claude");
        let mut cmds = Vec::new();
        let matcher = if use_chain {
            command.push_str(" --then ");
            command.push_str(then_cmd.as_deref().unwrap_or(DEFAULT_THEN));
            cmds.push(command);
            match ensure_shim() {
                Some(p) => {
                    cmds.push(p);
                    notes.push(
                        "chained to rtk: dtk handles tables first, rtk gets every other event; rtk init sees its hook as installed".to_string(),
                    );
                }
                None => notes.push(
                    "could not create the placeholder; `rtk init` may add a second hook"
                        .to_string(),
                ),
            }
            let mut parts = vec![OWN_MATCHER.to_string()];
            parts.extend(own_matchers);
            parts.extend(folded.iter().map(|f| f.0.clone()));
            union_tokens(&parts)
        } else {
            cmds.push(command);
            OWN_MATCHER.to_string()
        };
        post.push(entry(&matcher, &cmds));
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
