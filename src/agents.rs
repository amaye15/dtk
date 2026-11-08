//! `dtk init --agent NAME`: register dtk with an agent other than Claude Code.
//!
//! | NAME | what is written | mechanism |
//! |---|---|---|
//! | `codex` | `~/.codex/hooks.json` (or `$CODEX_HOME`) | `PostToolUse` hook |
//! | `gemini` | `~/.gemini/settings.json` | `AfterTool` hook |
//! | `copilot` | `~/.copilot/hooks/dtk.json` (or `$COPILOT_HOME`) | `postToolUse` hook |
//! | `opencode` | `~/.config/opencode/plugins/dtk.ts` | a plugin, `tool.execute.after` |
//! | `cursor` | `.cursor/rules/dtk.mdc` in this folder | a rule: Cursor's hooks cannot replace output |
//! | `rules` | a managed block in `AGENTS.md` (or `--file PATH`) | a rule, for any agent that reads one |
//!
//! A file that already holds other hooks keeps them: dtk's entry is added next to them, a backup
//! of the file is made once, and running the command again changes nothing. `--uninstall` takes
//! only dtk's entry out. Nothing here rewrites a command; see `harness`.

use crate::hook::home;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const OPENCODE_PLUGIN: &str = include_str!("../integrations/opencode/dtk.ts");

const BLOCK_START: &str = "<!-- dtk:start -->";
const BLOCK_END: &str = "<!-- dtk:end -->";

/// The advice that an agent without a usable hook is given in a rules file.
const RULE: &str = "## dtk: say data output in fewer tokens

When a shell command prints a table, CSV, JSON, JSON Lines, a log or a web page, run it as:

    dtk run -c '<the command>'

dtk prints a short summary of all of the output and keeps the full data on disk. It prints the
output unchanged when it is not data, and when you asked for exact lines (`head`, `tail`,
`sed -n`, `grep`). To read more of a summarized table, use the file or id that the summary names:
`dtk where FILE COLUMN = VALUE`, `dtk top FILE COLUMN`, `dtk rows FILE START`, `dtk get FILE PATH`,
`dtk html FILE WHAT`. Do not use dtk for source code, or when you need the exact text of a file.
";

fn exe() -> Option<String> {
    let me = std::env::current_exe().ok()?;
    Some(crate::init::command_path(&crate::init::stable_exe(
        &me,
        std::env::var_os("PATH").as_deref(),
    )))
}

fn env_dir(var: &str, default: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => home().map(|h| h.join(default)),
    }
}

fn codex_dir() -> Option<PathBuf> {
    env_dir("CODEX_HOME", ".codex")
}

fn gemini_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".gemini"))
}

fn copilot_dir() -> Option<PathBuf> {
    env_dir("COPILOT_HOME", ".copilot")
}

fn opencode_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d).join("opencode")),
        _ => home().map(|h| h.join(".config/opencode")),
    }
}

/// Is this handler dtk's for `agent`? (`.../dtk hook codex`)
fn is_ours(handler: &Value, agent: &str) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains("dtk") && c.contains(&format!("hook {agent}")))
}

fn group_is_ours(group: &Value, agent: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hs| !hs.is_empty() && hs.iter().all(|h| is_ours(h, agent)))
}

/// Reads a JSON file that must be an object (or does not exist yet).
fn read_object(path: &Path) -> Result<Value, String> {
    match fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(json!({})),
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) if v.is_object() => Ok(v),
            Ok(_) => Err(format!("{} is not a JSON object", path.display())),
            Err(e) => Err(format!("{} is not valid JSON ({e})", path.display())),
        },
        Err(_) => Ok(json!({})),
    }
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let backup = PathBuf::from(format!("{}.dtk-backup", path.display()));
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup).map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Adds dtk's group under `hooks.EVENT` of a JSON file that already holds other hooks, or removes
/// it. Returns what happened.
fn edit_hooks_file(
    path: &Path,
    event: &str,
    agent: &str,
    group: Option<Value>,
) -> Result<&'static str, String> {
    let mut root = read_object(path)?;
    let obj = root.as_object_mut().expect("an object");
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(format!("`hooks` in {} is not an object", path.display()));
    };
    let list = hooks.entry(event).or_insert_with(|| json!([]));
    let Some(list) = list.as_array_mut() else {
        return Err(format!(
            "`hooks.{event}` in {} is not a list",
            path.display()
        ));
    };
    let had = list.iter().any(|g| group_is_ours(g, agent));
    let before = list.clone();
    list.retain(|g| !group_is_ours(g, agent));
    let removed_only = group.is_none();
    if let Some(g) = group {
        list.push(g);
    }
    if list.is_empty() {
        hooks.remove(event);
    }
    if hooks.is_empty() {
        obj.remove("hooks");
    }
    let unchanged = root == read_object(path).unwrap_or(json!({})) && path.exists();
    if unchanged {
        return Ok("already");
    }
    if removed_only && !had {
        return Ok("absent");
    }
    let _ = before;
    write_json(path, &root)?;
    Ok(if removed_only {
        "removed"
    } else if had {
        "updated"
    } else {
        "added"
    })
}

fn say(agent: &str, what: &str, path: &Path) {
    eprintln!("dtk: {agent}: {what} {}", path.display());
}

fn fail(msg: &str) -> i32 {
    eprintln!("dtk: {msg}; not touching it");
    1
}

/// Takes a managed block out of `text`.
fn without_block(text: &str) -> String {
    match (text.find(BLOCK_START), text.find(BLOCK_END)) {
        (Some(a), Some(b)) if a < b => {
            let end = b + BLOCK_END.len();
            let mut out = String::new();
            out.push_str(text[..a].trim_end());
            let rest = text[end..].trim_start_matches('\n');
            if !out.is_empty() && !rest.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(rest);
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out
        }
        _ => text.to_string(),
    }
}

fn with_block(text: &str) -> String {
    let mut out = without_block(text);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
    out.push_str(&format!("{BLOCK_START}\n{RULE}{BLOCK_END}\n"));
    out
}

fn install_rules_block(path: &Path, uninstall: bool) -> i32 {
    let current = fs::read_to_string(path).unwrap_or_default();
    if uninstall {
        if !current.contains(BLOCK_START) {
            eprintln!("dtk: nothing to remove in {}", path.display());
            return 0;
        }
        let rest = without_block(&current);
        let r = if rest.trim().is_empty() {
            fs::remove_file(path)
        } else {
            fs::write(path, rest)
        };
        return match r {
            Ok(()) => {
                say("rules", "removed the block from", path);
                0
            }
            Err(e) => fail(&format!("cannot write {}: {e}", path.display())),
        };
    }
    let new = with_block(&current);
    if new == current {
        say("rules", "already up to date:", path);
        return 0;
    }
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    match fs::write(path, new) {
        Ok(()) => {
            say("rules", "wrote the block in", path);
            0
        }
        Err(e) => fail(&format!("cannot write {}: {e}", path.display())),
    }
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// `dtk init --agent NAME [--uninstall] [--file PATH]`
pub fn init(agent: &str, args: &[String]) -> i32 {
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let Some(exe) = exe() else {
        return fail("cannot find my own path");
    };
    match agent {
        "codex" => {
            let Some(dir) = codex_dir() else { return 1 };
            let path = dir.join("hooks.json");
            // No matcher: it is optional, and dtk answers at once for a tool that is not its own.
            let group = (!uninstall).then(|| {
                json!({"hooks": [{"type": "command", "command": format!("{exe} hook codex"),
                                  "timeout": 15, "statusMessage": "dtk"}]})
            });
            match edit_hooks_file(&path, "PostToolUse", "codex", group) {
                Ok(r) => {
                    say("codex", r, &path);
                    if !uninstall {
                        eprintln!("dtk: codex: Codex asks you to review a new hook in /hooks before it runs, and needs `hooks = true` under [features] in config.toml");
                    }
                    0
                }
                Err(e) => fail(&e),
            }
        }
        "gemini" => {
            let Some(dir) = gemini_dir() else { return 1 };
            let path = dir.join("settings.json");
            let group = (!uninstall).then(|| {
                json!({"matcher": "run_shell_command|mcp_.*",
                       "hooks": [{"type": "command", "command": format!("{exe} hook gemini"),
                                  "timeout": 15000, "name": "dtk"}]})
            });
            match edit_hooks_file(&path, "AfterTool", "gemini", group) {
                Ok(r) => {
                    say("gemini", r, &path);
                    0
                }
                Err(e) => fail(&e),
            }
        }
        "copilot" => {
            let Some(dir) = copilot_dir() else { return 1 };
            let path = dir.join("hooks").join("dtk.json");
            if uninstall {
                return match fs::remove_file(&path) {
                    Ok(()) => {
                        say("copilot", "removed", &path);
                        0
                    }
                    Err(_) => {
                        eprintln!("dtk: nothing to remove at {}", path.display());
                        0
                    }
                };
            }
            let cmd = format!("{exe} hook copilot");
            let v = json!({"version": 1, "hooks": {"postToolUse": [
                {"type": "command", "bash": cmd, "powershell": cmd, "timeoutSec": 15}]}});
            match write_json(&path, &v) {
                Ok(()) => {
                    say("copilot", "wrote", &path);
                    0
                }
                Err(e) => fail(&e),
            }
        }
        "opencode" => {
            let Some(dir) = opencode_dir() else { return 1 };
            let path = dir.join("plugins").join("dtk.ts");
            if uninstall {
                return match fs::remove_file(&path) {
                    Ok(()) => {
                        say("opencode", "removed", &path);
                        0
                    }
                    Err(_) => {
                        eprintln!("dtk: nothing to remove at {}", path.display());
                        0
                    }
                };
            }
            let _ = fs::create_dir_all(path.parent().expect("a folder"));
            if let Err(e) = fs::write(&path, OPENCODE_PLUGIN) {
                return fail(&format!("cannot write {}: {e}", path.display()));
            }
            say("opencode", "wrote", &path);
            if !crate::hook::dtk_on_path() {
                eprintln!("dtk: opencode: the plugin finds dtk on OpenCode's PATH, and `dtk` is not on yours; put it on the PATH (Homebrew and the install script do)");
            }
            0
        }
        "cursor" => {
            let path = PathBuf::from(".cursor/rules/dtk.mdc");
            if uninstall {
                return match fs::remove_file(&path) {
                    Ok(()) => {
                        say("cursor", "removed", &path);
                        0
                    }
                    Err(_) => {
                        eprintln!("dtk: nothing to remove at {}", path.display());
                        0
                    }
                };
            }
            let _ = fs::create_dir_all(".cursor/rules");
            let text = format!(
                "---\ndescription: Run commands that print data through dtk\nalwaysApply: true\n---\n\n{RULE}"
            );
            match fs::write(&path, text) {
                Ok(()) => {
                    say("cursor", "wrote", &path);
                    eprintln!("dtk: cursor: Cursor's hooks cannot replace a shell command's output, so this is a rule that the model follows; it is not automatic");
                    0
                }
                Err(e) => fail(&format!("cannot write {}: {e}", path.display())),
            }
        }
        "rules" => {
            let path =
                PathBuf::from(option_value(args, "--file").unwrap_or_else(|| "AGENTS.md".into()));
            install_rules_block(&path, uninstall)
        }
        other => {
            eprintln!("dtk init --agent: unknown agent {other}\nknown: claude, codex, gemini, copilot, opencode, cursor, rules");
            2
        }
    }
}

/// What `dtk doctor` says about the other agents: only for those that seem to be installed.
pub fn status() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let mut check = |name: &str,
                     dir: Option<PathBuf>,
                     registered: &dyn Fn(&Path) -> bool,
                     fix: &str| {
        let Some(dir) = dir.filter(|d| d.is_dir()) else {
            return;
        };
        if registered(&dir) {
            out.push((
                "ok",
                format!("{name}: dtk is registered ({})", dir.display()),
            ));
        } else {
            out.push((
                "--",
                format!("{name} is installed, and dtk is not registered with it. Run: dtk init --agent {fix}"),
            ));
        }
    };
    let in_hooks = |file: &'static str, event: &'static str, agent: &'static str| {
        move |dir: &Path| {
            read_object(&dir.join(file)).ok().is_some_and(|v| {
                v["hooks"][event]
                    .as_array()
                    .is_some_and(|l| l.iter().any(|g| group_is_ours(g, agent)))
            })
        }
    };
    check(
        "codex",
        codex_dir(),
        &in_hooks("hooks.json", "PostToolUse", "codex"),
        "codex",
    );
    check(
        "gemini",
        gemini_dir(),
        &in_hooks("settings.json", "AfterTool", "gemini"),
        "gemini",
    );
    check(
        "copilot",
        copilot_dir(),
        &|d| d.join("hooks").join("dtk.json").is_file(),
        "copilot",
    );
    check(
        "opencode",
        opencode_dir(),
        &|d| d.join("plugins").join("dtk.ts").is_file(),
        "opencode",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_added_replaced_and_taken_out_without_touching_the_rest() {
        let doc = "# My rules\n\nBe brief.\n";
        let with = with_block(doc);
        assert!(
            with.starts_with("# My rules\n\nBe brief.\n\n<!-- dtk:start -->"),
            "{with}"
        );
        assert_eq!(with_block(&with), with, "twice is once");
        assert_eq!(without_block(&with), doc);
        assert_eq!(without_block(&with_block("")), "");
        // Text after the block stays.
        let after = format!("{with}\nMore rules.\n");
        assert_eq!(
            without_block(&after),
            "# My rules\n\nBe brief.\n\nMore rules.\n"
        );
    }

    #[test]
    fn a_handler_is_dtks_only_when_it_runs_dtk_hook_for_that_agent() {
        let ours = json!({"type": "command", "command": "/usr/local/bin/dtk hook codex"});
        assert!(is_ours(&ours, "codex"));
        assert!(!is_ours(&ours, "gemini"));
        let remem = json!({"type": "command", "command": "/opt/homebrew/bin/remem context --host codex-cli"});
        assert!(!is_ours(&remem, "codex"));
        assert!(
            !group_is_ours(&json!({"hooks": [ours.clone(), remem]}), "codex"),
            "a group with another hook in it is not ours"
        );
        assert!(group_is_ours(&json!({"hooks": [ours]}), "codex"));
    }
}
