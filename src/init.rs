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
const OWN_MATCHER: &str = "Bash|Read|mcp__.*";

pub fn settings_path() -> Option<PathBuf> {
    Some(home()?.join(".claude/settings.json"))
}

fn state_path() -> Option<PathBuf> {
    Some(config_dir()?.join("init-state.json"))
}

pub fn shim_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "rtk.exe" } else { "rtk" };
    Some(data_dir()?.join("shim").join(name))
}

/// A path as it goes into a hook command: forward slashes (a bash-run hook would eat
/// backslashes, and Windows accepts forward slashes), and quoted when it has a space.
pub fn command_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    let s = if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    };
    if s.contains(char::is_whitespace) || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s
    }
}

/// The path to put in the hook. On Linux the operating system gives the real path of a program,
/// and with Homebrew that is a folder named after the version (`.../Cellar/dtk/0.11.0/bin/dtk`).
/// The next upgrade deletes it, and the hook would point at nothing. A folder on the PATH that
/// holds the same file under a stable name (`/home/linuxbrew/.linuxbrew/bin/dtk`) survives.
pub fn stable_exe(exe: &std::path::Path, path_var: Option<&std::ffi::OsStr>) -> PathBuf {
    let Ok(real) = exe.canonicalize() else {
        return exe.to_path_buf();
    };
    let name = exe.file_name().unwrap_or_default();
    for dir in path_var.map(std::env::split_paths).into_iter().flatten() {
        let candidate = dir.join(name);
        if candidate.is_absolute() && candidate.canonicalize().is_ok_and(|c| c == real) {
            return candidate;
        }
    }
    exe.to_path_buf()
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
    let c = cmd.replace('"', "").replace('\\', "/");
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

/// Creates the placeholder and returns its hook command.
///
/// The placeholder is a copy of dtk itself under the name `rtk`. When dtk is started under that
/// name with `hook claude`, it does nothing and exits (see `main`). That works the same on every
/// system, with no shell script, and rtk's own check only looks at the shape of the command.
fn ensure_shim() -> Option<String> {
    let p = shim_path()?;
    create_private_dir(p.parent()?).ok()?;
    let exe = std::env::current_exe().ok()?;
    let fresh = fs::metadata(&p)
        .ok()
        .zip(fs::metadata(&exe).ok())
        .is_some_and(|(a, b)| a.len() == b.len() && a.modified().ok() >= b.modified().ok());
    if !fresh {
        let _ = fs::remove_file(&p);
        fs::copy(&exe, &p).ok()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(format!("{} hook claude", command_path(&p)))
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

/// How to remove the program, for the way it was installed (read from where the binary is).
pub fn removal_hint(exe: &std::path::Path) -> String {
    let p = exe.display().to_string().replace('\\', "/");
    let lower = p.to_ascii_lowercase();
    if lower.contains("/cellar/") || lower.contains("/homebrew/") || lower.contains("/linuxbrew/") {
        "brew uninstall dtk".to_string()
    } else if lower.contains("/.cargo/bin/") {
        "cargo uninstall dtk".to_string()
    } else if lower.contains("/scoop/") {
        "scoop uninstall dtk".to_string()
    } else if lower.contains("/winget/") || lower.contains("/windowsapps/") {
        "winget uninstall amaye15.dtk".to_string()
    } else if lower.starts_with("/usr/bin/") {
        "sudo apt remove dtk   (Debian, Ubuntu)  or  sudo rpm -e dtk   (Fedora, RHEL)".to_string()
    } else {
        format!("delete the file {p}")
    }
}

pub fn run(args: &[String]) -> i32 {
    // Another agent: `dtk init --agent codex`.
    if let Some(i) = args.iter().position(|a| a == "--agent") {
        let Some(agent) = args.get(i + 1).map(String::as_str) else {
            eprintln!("usage: dtk init --agent claude|codex|gemini|copilot|vibe|opencode|kilo|pi|hermes|cursor|rules");
            return 2;
        };
        if agent == "claude" {
            let rest: Vec<String> = args
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != i && *k != i + 1)
                .map(|(_, a)| a.clone())
                .collect();
            return run(&rest);
        }
        return crate::agents::init(agent, args);
    }
    let uninstall = args.iter().any(|a| a == "--uninstall");
    if !args.iter().any(|a| a == "--global") && !uninstall {
        eprintln!(
            "usage: dtk init --global [--no-rtk] [--claude-only] | dtk init --uninstall [--purge]\n       dtk init --agent NAME [--uninstall]"
        );
        return 2;
    }
    // `--global` connects Claude Code and every other agent that dtk finds on this machine.
    // `--uninstall` leaves every agent that has dtk in it. `--claude-only` keeps to Claude Code.
    let others = if args.iter().any(|a| a == "--claude-only") {
        Vec::new()
    } else {
        crate::agents::found(uninstall)
    };
    let claude_here = crate::hook::home().is_some_and(|h| h.join(".claude").is_dir());
    let mut code = 0;
    if uninstall || claude_here || others.is_empty() {
        code = run_claude(args);
    } else {
        println!("dtk: Claude Code was not found (no ~/.claude folder), so it is left out");
    }
    if !others.is_empty() {
        println!(
            "dtk: {} {}",
            if uninstall {
                "leaving the agents that have dtk:"
            } else {
                "other agents found:"
            },
            others.join(", ")
        );
        code = code.max(crate::agents::init_all(&others, uninstall));
        if !uninstall {
            println!("dtk: restart each agent (or start a new session) to load dtk");
        }
    }
    code
}

/// Claude Code: the hook in `~/.claude/settings.json`, chained to rtk.
fn run_claude(args: &[String]) -> i32 {
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let no_rtk = args.iter().any(|a| a == "--no-rtk");
    let Some(path) = settings_path() else {
        return 1;
    };
    let exe = match std::env::current_exe() {
        Ok(p) => command_path(&stable_exe(&p, std::env::var_os("PATH").as_deref())),
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
                // `mcp__.*` is dtk's own addition; rtk's hook is given back as rtk had it.
                let matcher = union_tokens(&matchers)
                    .split('|')
                    .filter(|t| *t != "mcp__.*")
                    .collect::<Vec<_>>()
                    .join("|");
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
        if args.iter().any(|a| a == "--purge") {
            for dir in [data_dir(), config_dir()].into_iter().flatten() {
                if dir.exists() && fs::remove_dir_all(&dir).is_ok() {
                    println!("dtk: removed {}", dir.display());
                }
            }
        } else {
            println!("dtk: dtk's data and config are kept; add --purge to remove them too");
        }
        if let Ok(exe) = std::env::current_exe() {
            println!("dtk: to remove the program itself: {}", removal_hint(&exe));
        }
    } else {
        println!(
            "dtk: hook registered in {} (backup: {})",
            path.display(),
            backup.display()
        );
        println!("dtk: restart Claude Code (or start a new session) to load it");
        // `--claude-only`: say which other agents on this machine have no dtk.
        if args.iter().any(|a| a == "--claude-only") {
            for (tag, text) in crate::agents::status() {
                if tag == "--" {
                    println!("dtk: {text}");
                }
            }
        }
    }
    for n in notes {
        println!("dtk: {n}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn command_paths_are_quoted_only_when_needed() {
        assert_eq!(
            command_path(Path::new("/usr/local/bin/dtk")),
            "/usr/local/bin/dtk"
        );
        assert_eq!(
            command_path(Path::new("/Users/some one/bin/dtk")),
            "\"/Users/some one/bin/dtk\""
        );
    }

    #[test]
    fn the_removal_hint_matches_how_dtk_was_installed() {
        let h = |p: &str| removal_hint(Path::new(p));
        assert_eq!(
            h("/opt/homebrew/Cellar/dtk/0.4.0/bin/dtk"),
            "brew uninstall dtk"
        );
        assert_eq!(h("/opt/homebrew/bin/dtk"), "brew uninstall dtk");
        assert_eq!(
            h("/home/linuxbrew/.linuxbrew/bin/dtk"),
            "brew uninstall dtk"
        );
        assert_eq!(h("/Users/me/.cargo/bin/dtk"), "cargo uninstall dtk");
        assert_eq!(
            h("C:\\Users\\me\\scoop\\apps\\dtk\\current\\dtk.exe"),
            "scoop uninstall dtk"
        );
        assert_eq!(
            h("C:\\Users\\me\\AppData\\Local\\Microsoft\\WinGet\\Links\\dtk.exe"),
            "winget uninstall amaye15.dtk"
        );
        assert!(h("/usr/bin/dtk").contains("apt remove dtk"));
        assert_eq!(
            h("/Users/me/.local/bin/dtk"),
            "delete the file /Users/me/.local/bin/dtk"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_hook_names_a_path_that_survives_an_upgrade() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("dtk-stable-{}", std::process::id()));
        let cellar = root.join("Cellar/dtk/0.11.0/bin");
        let bin = root.join("bin");
        fs::create_dir_all(&cellar).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(cellar.join("dtk"), b"x").unwrap();
        symlink(cellar.join("dtk"), bin.join("dtk")).unwrap();
        // The operating system reports the versioned file; the folder on the PATH is stable.
        let real = cellar.join("dtk").canonicalize().unwrap();
        let path = std::env::join_paths([root.join("nowhere"), bin.clone()]).unwrap();
        assert_eq!(stable_exe(&real, Some(path.as_os_str())), bin.join("dtk"));
        // Without a matching folder on the PATH the real path stays.
        assert_eq!(stable_exe(&real, None), real);
        let other = std::env::join_paths([root.join("nowhere")]).unwrap();
        assert_eq!(stable_exe(&real, Some(other.as_os_str())), real);
        // A different file with the same name is not the same program.
        let impostor = root.join("other");
        fs::create_dir_all(&impostor).unwrap();
        fs::write(impostor.join("dtk"), b"y").unwrap();
        let path = std::env::join_paths([impostor.clone()]).unwrap();
        assert_eq!(stable_exe(&real, Some(path.as_os_str())), real);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_placeholder_is_recognised_however_it_is_written() {
        assert!(is_placeholder(
            "/home/u/.local/share/dtk/shim/rtk hook claude"
        ));
        assert!(is_placeholder(
            "\"/home/some one/.local/share/dtk/shim/rtk\" hook claude"
        ));
        assert!(is_placeholder(
            "C:/Users/u/.local/share/dtk/shim/rtk.exe hook claude"
        ));
        assert!(is_placeholder(
            "C:\\Users\\u\\.local\\share\\dtk\\shim\\rtk.exe hook claude"
        ));
        assert!(!is_placeholder("rtk hook claude"));
        assert!(!is_placeholder(
            "/home/u/.cargo/bin/dtk hook claude --then rtk hook claude"
        ));
    }
}
