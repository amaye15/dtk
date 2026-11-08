//! `dtk init --agent NAME`: register dtk with an agent other than Claude Code.
//!
//! | NAME | what is written | mechanism |
//! |---|---|---|
//! | `codex` | `~/.codex/hooks.json` (or `$CODEX_HOME`) | `PostToolUse` hook |
//! | `gemini` | `~/.gemini/settings.json` | `AfterTool` hook |
//! | `copilot` | `~/.copilot/hooks/dtk.json` (or `$COPILOT_HOME`) | `postToolUse` hook |
//! | `vibe` | `~/.vibe/hooks.toml` (or `$VIBE_HOME`) | a `post_tool` hook, in a block that dtk manages |
//! | `opencode` | `~/.config/opencode/plugins/dtk.ts` | a plugin, `tool.execute.after` |
//! | `kilo` | `~/.config/kilo/plugin/dtk.ts` | the same plugin (Kilo Code is a fork of OpenCode) |
//! | `pi` | `~/.pi/agent/extensions/dtk.ts` | an extension, `tool_result` |
//! | `hermes` | `~/.hermes/plugins/dtk/` (or `$HERMES_HOME`) | a plugin, `transform_tool_result` |
//! | `cursor` | `.cursor/rules/dtk.mdc` in this folder | a rule: Cursor's hooks cannot replace output |
//! | `rules` | a managed block in `AGENTS.md` (or `--file PATH`) | a rule, for any agent that reads one |
//!
//! `dtk init --global` runs this for every agent of the list that it finds on the machine (see
//! `found`), after it has done Claude Code. rtk needs nothing here: in these agents rtk only has
//! hooks that run before a tool, and dtk's run after it, so the two never meet.
//!
//! A file that already holds other hooks keeps them: dtk's entry is added next to them, a backup
//! of the file is made once, and running the command again changes nothing. `--uninstall` takes
//! only dtk's entry out. Nothing here rewrites a command; see `harness`.

use crate::hook::home;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const OPENCODE_PLUGIN: &str = include_str!("../integrations/opencode/dtk.ts");
const PI_EXTENSION: &str = include_str!("../integrations/pi/dtk.ts");
const HERMES_PLUGIN: &str = include_str!("../integrations/hermes/__init__.py");
const HERMES_MANIFEST: &str = include_str!("../integrations/hermes/plugin.yaml");

/// The two lines that mark a block of text that dtk manages inside a file of the user.
#[derive(Clone, Copy)]
struct Marks {
    start: &'static str,
    end: &'static str,
}

const BLOCK_START: &str = "<!-- dtk:start -->";
const BLOCK_END: &str = "<!-- dtk:end -->";
const MARKDOWN: Marks = Marks {
    start: BLOCK_START,
    end: BLOCK_END,
};
const TOML: Marks = Marks {
    start: "# dtk:start",
    end: "# dtk:end",
};

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

fn xdg_dir(name: &str) -> Option<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d).join(name)),
        _ => home().map(|h| h.join(".config").join(name)),
    }
}

fn opencode_dir() -> Option<PathBuf> {
    xdg_dir("opencode")
}

fn kilo_dir() -> Option<PathBuf> {
    xdg_dir("kilo")
}

fn pi_dir() -> Option<PathBuf> {
    home().map(|h| h.join(".pi").join("agent"))
}

fn hermes_dir() -> Option<PathBuf> {
    env_dir("HERMES_HOME", ".hermes")
}

fn vibe_dir() -> Option<PathBuf> {
    env_dir("VIBE_HOME", ".vibe")
}

/// Is there a program of this name on the PATH?
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    std::env::split_paths(&path)
        .any(|d| exts.iter().any(|e| d.join(format!("{name}{e}")).is_file()))
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
        return Ok("dtk is already in");
    }
    if removed_only && !had {
        return Ok("dtk was not in");
    }
    let _ = before;
    write_json(path, &root)?;
    Ok(if removed_only {
        "removed dtk from"
    } else if had {
        "updated dtk in"
    } else {
        "added dtk to"
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
fn without_marks(text: &str, m: Marks) -> String {
    match (text.find(m.start), text.find(m.end)) {
        (Some(a), Some(b)) if a < b => {
            let end = b + m.end.len();
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

/// Puts the block (or the new version of it) at the end of `text`.
fn with_marks(text: &str, m: Marks, body: &str) -> String {
    let mut out = without_marks(text, m);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
    out.push_str(&format!("{}\n{body}{}\n", m.start, m.end));
    out
}

#[cfg(test)]
fn without_block(text: &str) -> String {
    without_marks(text, MARKDOWN)
}

#[cfg(test)]
fn with_block(text: &str) -> String {
    with_marks(text, MARKDOWN, RULE)
}

/// Writes (or takes out) a block that dtk manages in a text file of the user.
fn install_block(label: &str, path: &Path, m: Marks, body: &str, uninstall: bool) -> i32 {
    let current = fs::read_to_string(path).unwrap_or_default();
    if uninstall {
        if !current.contains(m.start) {
            eprintln!("dtk: nothing to remove in {}", path.display());
            return 0;
        }
        let rest = without_marks(&current, m);
        let r = if rest.trim().is_empty() {
            fs::remove_file(path)
        } else {
            fs::write(path, rest)
        };
        return match r {
            Ok(()) => {
                say(label, "removed the block from", path);
                0
            }
            Err(e) => fail(&format!("cannot write {}: {e}", path.display())),
        };
    }
    let new = with_marks(&current, m, body);
    if new == current {
        say(label, "already up to date:", path);
        return 0;
    }
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let backup = PathBuf::from(format!("{}.dtk-backup", path.display()));
    if path.exists() && !backup.exists() {
        let _ = fs::copy(path, &backup);
    }
    match fs::write(path, new) {
        Ok(()) => {
            say(label, "wrote the block in", path);
            0
        }
        Err(e) => fail(&format!("cannot write {}: {e}", path.display())),
    }
}

fn install_rules_block(path: &Path, uninstall: bool) -> i32 {
    install_block("rules", path, MARKDOWN, RULE, uninstall)
}

/// Writes one file (a plugin or an extension), or takes it away.
fn install_file(label: &str, path: &Path, content: &str, uninstall: bool) -> i32 {
    if uninstall {
        return match fs::remove_file(path) {
            Ok(()) => {
                say(label, "removed", path);
                0
            }
            Err(_) => {
                eprintln!("dtk: nothing to remove at {}", path.display());
                0
            }
        };
    }
    if fs::read_to_string(path).is_ok_and(|t| t == content) {
        say(label, "already up to date:", path);
        return 0;
    }
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Err(e) = fs::write(path, content) {
        return fail(&format!("cannot write {}: {e}", path.display()));
    }
    say(label, "wrote", path);
    0
}

/// The plugins and extensions find `dtk` on the PATH of their own agent. Said once per run.
fn warn_if_not_on_path() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    if !crate::hook::dtk_on_path() && !SAID.swap(true, Ordering::Relaxed) {
        eprintln!("dtk: the plugins and extensions find `dtk` on the PATH of their agent, and `dtk` is not on yours; put it on the PATH (Homebrew and the install script do)");
    }
}

/// Runs a program of another tool with a time limit. True when it ended with success.
fn run_quietly(program: &str, args: &[&str]) -> bool {
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < std::time::Duration::from_secs(15) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                return false;
            }
        }
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
            let cmd = format!("{exe} hook copilot");
            let v = json!({"version": 1, "hooks": {"postToolUse": [
                {"type": "command", "bash": cmd, "powershell": cmd, "timeoutSec": 15}]}});
            let text = serde_json::to_string_pretty(&v).expect("json") + "\n";
            install_file("copilot", &path, &text, uninstall)
        }
        "vibe" => {
            let Some(dir) = vibe_dir() else { return 1 };
            let cmd = serde_json::to_string(&format!("{exe} hook vibe")).expect("a string");
            let body = format!(
                "# Managed by dtk. Take it out with: dtk init --uninstall\n[[hooks]]\nname = \"dtk\"\ntype = \"post_tool\"\nmatch = \"bash\"\ncommand = {cmd}\ntimeout = 15\n"
            );
            install_block("vibe", &dir.join("hooks.toml"), TOML, &body, uninstall)
        }
        "opencode" | "kilo" | "pi" => {
            let (dir, sub, content) = match agent {
                "opencode" => (opencode_dir(), "plugins", OPENCODE_PLUGIN),
                "kilo" => (kilo_dir(), "plugin", OPENCODE_PLUGIN),
                _ => (pi_dir(), "extensions", PI_EXTENSION),
            };
            let Some(dir) = dir else { return 1 };
            let path = dir.join(sub).join("dtk.ts");
            let code = install_file(agent, &path, content, uninstall);
            if !uninstall && code == 0 {
                warn_if_not_on_path();
            }
            code
        }
        "hermes" => {
            let Some(dir) = hermes_dir() else { return 1 };
            let folder = dir.join("plugins").join("dtk");
            if uninstall {
                let code = install_file("hermes", &folder.join("__init__.py"), "", true)
                    + install_file("hermes", &folder.join("plugin.yaml"), "", true);
                let _ = fs::remove_dir(&folder);
                if on_path("hermes") {
                    run_quietly("hermes", &["plugins", "disable", "dtk"]);
                }
                return i32::from(code != 0);
            }
            let code =
                install_file(
                    "hermes",
                    &folder.join("plugin.yaml"),
                    HERMES_MANIFEST,
                    false,
                ) + install_file("hermes", &folder.join("__init__.py"), HERMES_PLUGIN, false);
            if code != 0 {
                return 1;
            }
            // Hermes loads a plugin of the user only after it is enabled.
            if on_path("hermes") && run_quietly("hermes", &["plugins", "enable", "dtk"]) {
                eprintln!("dtk: hermes: enabled the plugin (hermes plugins enable dtk)");
            } else {
                eprintln!("dtk: hermes: Hermes loads the plugin only when it is enabled. Run: hermes plugins enable dtk");
            }
            warn_if_not_on_path();
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
            eprintln!("dtk init --agent: unknown agent {other}\nknown: claude, codex, gemini, copilot, vibe, opencode, kilo, pi, hermes, cursor, rules");
            2
        }
    }
}

/// An agent that dtk can connect with by itself: where it keeps its files, the program that it is
/// (only when no other well-known program has the same name: `kilo` is also a text editor, and
/// `hermes` is also a JavaScript engine), and how to tell that dtk is in it.
struct Spec {
    name: &'static str,
    dir: fn() -> Option<PathBuf>,
    program: Option<&'static str>,
    registered: fn(&Path) -> bool,
}

fn in_hooks(dir: &Path, file: &str, event: &str, agent: &str) -> bool {
    read_object(&dir.join(file)).ok().is_some_and(|v| {
        v["hooks"][event]
            .as_array()
            .is_some_and(|l| l.iter().any(|g| group_is_ours(g, agent)))
    })
}

fn specs() -> Vec<Spec> {
    vec![
        Spec {
            name: "codex",
            dir: codex_dir,
            program: Some("codex"),
            registered: |d| in_hooks(d, "hooks.json", "PostToolUse", "codex"),
        },
        Spec {
            name: "gemini",
            dir: gemini_dir,
            program: Some("gemini"),
            registered: |d| in_hooks(d, "settings.json", "AfterTool", "gemini"),
        },
        Spec {
            name: "copilot",
            dir: copilot_dir,
            program: Some("copilot"),
            registered: |d| d.join("hooks").join("dtk.json").is_file(),
        },
        Spec {
            name: "vibe",
            dir: vibe_dir,
            program: None,
            registered: |d| {
                fs::read_to_string(d.join("hooks.toml")).is_ok_and(|t| t.contains(TOML.start))
            },
        },
        Spec {
            name: "opencode",
            dir: opencode_dir,
            program: Some("opencode"),
            registered: |d| d.join("plugins").join("dtk.ts").is_file(),
        },
        Spec {
            name: "kilo",
            dir: kilo_dir,
            program: None,
            registered: |d| d.join("plugin").join("dtk.ts").is_file(),
        },
        Spec {
            name: "pi",
            dir: pi_dir,
            program: None,
            registered: |d| d.join("extensions").join("dtk.ts").is_file(),
        },
        Spec {
            name: "hermes",
            dir: hermes_dir,
            program: None,
            registered: |d| d.join("plugins").join("dtk").join("__init__.py").is_file(),
        },
    ]
}

/// The agents that `dtk init --global` connects, or (for `--uninstall`) leaves. To connect: the
/// agent has a folder here, or its program is on the PATH. To leave: dtk is registered in it.
pub fn found(uninstall: bool) -> Vec<&'static str> {
    specs()
        .into_iter()
        .filter(|s| {
            let dir = (s.dir)().filter(|d| d.is_dir());
            if uninstall {
                dir.is_some_and(|d| (s.registered)(&d))
            } else {
                dir.is_some() || s.program.is_some_and(on_path)
            }
        })
        .map(|s| s.name)
        .collect()
}

/// Connects (or leaves) each of `names`. The worst exit code: one agent that cannot be changed
/// (a settings file with a mistake in it) does not stop the others.
pub fn init_all(names: &[&str], uninstall: bool) -> i32 {
    let args: Vec<String> = if uninstall {
        vec!["--uninstall".into()]
    } else {
        Vec::new()
    };
    names
        .iter()
        .map(|n| init(n, &args))
        .max()
        .unwrap_or_default()
}

/// What `dtk doctor` says about the other agents: only for those that seem to be installed.
pub fn status() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for s in specs() {
        let dir = (s.dir)().filter(|d| d.is_dir());
        match dir {
            Some(d) if (s.registered)(&d) => out.push((
                "ok",
                format!("{}: dtk is registered ({})", s.name, d.display()),
            )),
            Some(_) => out.push((
                "--",
                format!(
                    "{} is installed, and dtk is not registered with it. Run: dtk init --global",
                    s.name
                ),
            )),
            None if s.program.is_some_and(on_path) => out.push((
                "--",
                format!(
                    "{} is on the PATH, and dtk is not registered with it. Run: dtk init --global",
                    s.name
                ),
            )),
            None => {}
        }
    }
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
