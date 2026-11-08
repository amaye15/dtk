//! `dtk init --agent NAME`: the files that dtk writes for the other agents, and what it must keep.
//!
//! The most important rule: a file that already holds other hooks (a memory tool's, for example)
//! keeps them. dtk's entry is added next to them, once, and `--uninstall` takes out only its own.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_dtk");

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-agents-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("CODEX_HOME")
        .env_remove("COPILOT_HOME")
        .env_remove("HERMES_HOME")
        .env_remove("VIBE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap()
}

fn json_of(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn remem_codex_hooks() -> Value {
    json!({"hooks": {
        "SessionStart": [{"hooks": [{"command": "/opt/homebrew/bin/remem context --host codex-cli", "timeout": 15, "type": "command"}]}],
        "Stop": [{"hooks": [{"command": "/opt/homebrew/bin/remem summarize --host codex-cli", "timeout": 120, "type": "command"}]}]
    }})
}

#[test]
fn codex_keeps_the_hooks_it_has_and_is_added_once_and_removed_alone() {
    let h = home("codex");
    fs::create_dir_all(h.join(".codex")).unwrap();
    let path = h.join(".codex/hooks.json");
    fs::write(
        &path,
        serde_json::to_string_pretty(&remem_codex_hooks()).unwrap(),
    )
    .unwrap();

    let o = dtk(&h, &h, &["init", "--agent", "codex"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v = json_of(&path);
    // The other hooks are as they were.
    assert_eq!(
        v["hooks"]["SessionStart"],
        remem_codex_hooks()["hooks"]["SessionStart"]
    );
    assert_eq!(v["hooks"]["Stop"], remem_codex_hooks()["hooks"]["Stop"]);
    // dtk's is there, once, and runs `dtk hook codex`.
    let post = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1);
    let command = post[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(command.ends_with("hook codex"), "{command}");
    assert_eq!(post[0]["hooks"][0]["type"], "command");
    // A backup of the file as it was.
    assert_eq!(
        json_of(&h.join(".codex/hooks.json.dtk-backup")),
        remem_codex_hooks()
    );

    // Again: no second entry, nothing else changes.
    dtk(&h, &h, &["init", "--agent", "codex"]);
    assert_eq!(
        json_of(&path)["hooks"]["PostToolUse"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Out: only dtk's entry goes, and the file is as it was.
    let o = dtk(&h, &h, &["init", "--agent", "codex", "--uninstall"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(json_of(&path), remem_codex_hooks());
}

#[test]
fn codex_is_made_when_there_is_no_file_and_a_broken_file_is_not_touched() {
    let h = home("codex-new");
    let o = dtk(&h, &h, &["init", "--agent", "codex"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(
        json_of(&h.join(".codex/hooks.json"))["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("hook codex")
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("/hooks"),
        "the review that Codex asks for is said"
    );

    let h = home("codex-broken");
    fs::create_dir_all(h.join(".codex")).unwrap();
    fs::write(h.join(".codex/hooks.json"), "{ not json").unwrap();
    let o = dtk(&h, &h, &["init", "--agent", "codex"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(h.join(".codex/hooks.json")).unwrap(),
        "{ not json"
    );
}

#[test]
fn gemini_keeps_its_settings_and_gets_a_matcher_for_shell_and_mcp_tools() {
    let h = home("gemini");
    fs::create_dir_all(h.join(".gemini")).unwrap();
    let before = json!({"theme": "dark", "mcpServers": {"db": {"command": "db-mcp"}},
                        "hooks": {"BeforeTool": [{"matcher": "write_file", "hooks": [{"type": "command", "command": "./audit.sh"}]}]}});
    fs::write(h.join(".gemini/settings.json"), before.to_string()).unwrap();
    dtk(&h, &h, &["init", "--agent", "gemini"]);
    let v = json_of(&h.join(".gemini/settings.json"));
    assert_eq!(v["theme"], "dark");
    assert_eq!(v["mcpServers"], before["mcpServers"]);
    assert_eq!(v["hooks"]["BeforeTool"], before["hooks"]["BeforeTool"]);
    let g = &v["hooks"]["AfterTool"][0];
    assert_eq!(g["matcher"], "run_shell_command|mcp_.*");
    assert!(g["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("hook gemini"));
    assert_eq!(g["hooks"][0]["timeout"], 15000);
    dtk(&h, &h, &["init", "--agent", "gemini", "--uninstall"]);
    assert_eq!(json_of(&h.join(".gemini/settings.json")), before);
}

#[test]
fn copilot_and_opencode_get_a_file_of_their_own_that_uninstall_removes() {
    let h = home("copilot-opencode");
    dtk(&h, &h, &["init", "--agent", "copilot"]);
    let p = h.join(".copilot/hooks/dtk.json");
    let v = json_of(&p);
    assert_eq!(v["version"], 1);
    let hook = &v["hooks"]["postToolUse"][0];
    assert!(hook["bash"].as_str().unwrap().ends_with("hook copilot"));
    assert_eq!(hook["timeoutSec"], 15);
    dtk(&h, &h, &["init", "--agent", "copilot", "--uninstall"]);
    assert!(!p.exists());

    dtk(&h, &h, &["init", "--agent", "opencode"]);
    let plugin = h.join(".config/opencode/plugins/dtk.ts");
    let src = fs::read_to_string(&plugin).unwrap();
    assert!(
        src.contains("tool.execute.after") && src.contains("hook\", \"opencode"),
        "the plugin of the repository"
    );
    assert_eq!(
        src,
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("integrations/opencode/dtk.ts")
        )
        .unwrap()
    );
    dtk(&h, &h, &["init", "--agent", "opencode", "--uninstall"]);
    assert!(!plugin.exists());
}

#[test]
fn a_rules_block_goes_in_beside_what_is_there_and_comes_out_alone() {
    let h = home("rules");
    let work = h.join("project");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("AGENTS.md"), "# Project rules\n\nUse tabs.\n").unwrap();
    dtk(&h, &work, &["init", "--agent", "rules"]);
    let text = fs::read_to_string(work.join("AGENTS.md")).unwrap();
    assert!(text.starts_with("# Project rules\n\nUse tabs.\n"), "{text}");
    assert!(text.contains("dtk run -c '<the command>'"), "{text}");
    dtk(&h, &work, &["init", "--agent", "rules"]);
    assert_eq!(
        fs::read_to_string(work.join("AGENTS.md")).unwrap(),
        text,
        "twice is once"
    );
    dtk(&h, &work, &["init", "--agent", "rules", "--uninstall"]);
    assert_eq!(
        fs::read_to_string(work.join("AGENTS.md")).unwrap(),
        "# Project rules\n\nUse tabs.\n"
    );

    // A file that dtk made alone is removed with the block.
    dtk(
        &h,
        &work,
        &["init", "--agent", "rules", "--file", "CONVENTIONS.md"],
    );
    assert!(work.join("CONVENTIONS.md").exists());
    dtk(
        &h,
        &work,
        &[
            "init",
            "--agent",
            "rules",
            "--file",
            "CONVENTIONS.md",
            "--uninstall",
        ],
    );
    assert!(!work.join("CONVENTIONS.md").exists());

    // Cursor: its hooks cannot replace a shell output, so it is a rule, and it says so.
    let o = dtk(&h, &work, &["init", "--agent", "cursor"]);
    let mdc = fs::read_to_string(work.join(".cursor/rules/dtk.mdc")).unwrap();
    assert!(
        mdc.starts_with("---\n") && mdc.contains("alwaysApply: true"),
        "{mdc}"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("not automatic"));
}

#[test]
fn an_unknown_agent_is_refused_and_claude_still_goes_the_old_way() {
    let h = home("unknown");
    let o = dtk(&h, &h, &["init", "--agent", "nosuchagent"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("known: claude, codex"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("vibe, opencode, kilo, pi, hermes"));
    let o = dtk(&h, &h, &["init", "--agent"]);
    assert_eq!(o.status.code(), Some(2));
    // `--agent claude --global` is `--global`.
    let o = dtk(&h, &h, &["init", "--agent", "claude", "--global"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(h.join(".claude/settings.json").exists());
}

#[test]
fn doctor_names_an_installed_agent_that_dtk_is_or_is_not_registered_with() {
    let h = home("doctor");
    fs::create_dir_all(h.join(".codex")).unwrap();
    let said = |h: &Path| String::from_utf8_lossy(&dtk(h, h, &["doctor"]).stdout).to_string();
    let before = said(&h);
    assert!(
        before.contains(
            "codex is installed, and dtk is not registered with it. Run: dtk init --global"
        ),
        "{before}"
    );
    assert!(
        !before.contains("gemini"),
        "an agent that is not installed is not mentioned:\n{before}"
    );
    dtk(&h, &h, &["init", "--agent", "codex"]);
    assert!(
        said(&h).contains("codex: dtk is registered"),
        "{}",
        said(&h)
    );
}

// ---- more agents: a managed block, a plugin or an extension -----------------------------------------

#[test]
fn vibe_gets_a_block_in_its_hooks_file_beside_the_hooks_that_are_there() {
    let h = home("vibe");
    fs::create_dir_all(h.join(".vibe")).unwrap();
    let before = "[[hooks]]\nname = \"audit\"\ntype = \"pre_tool\"\ncommand = \"./audit.sh\"\n";
    fs::write(h.join(".vibe/hooks.toml"), before).unwrap();
    let o = dtk(&h, &h, &["init", "--agent", "vibe"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = fs::read_to_string(h.join(".vibe/hooks.toml")).unwrap();
    assert!(text.starts_with(before), "{text}");
    assert!(text.contains("type = \"post_tool\""), "{text}");
    assert!(text.contains("match = \"bash\""), "{text}");
    assert!(text.contains("hook vibe\""), "{text}");
    assert_eq!(text.matches("[[hooks]]").count(), 2, "{text}");
    assert_eq!(
        fs::read_to_string(h.join(".vibe/hooks.toml.dtk-backup")).unwrap(),
        before
    );
    // Twice is once. Out: the file is as it was.
    dtk(&h, &h, &["init", "--agent", "vibe"]);
    assert_eq!(
        fs::read_to_string(h.join(".vibe/hooks.toml")).unwrap(),
        text
    );
    dtk(&h, &h, &["init", "--agent", "vibe", "--uninstall"]);
    assert_eq!(
        fs::read_to_string(h.join(".vibe/hooks.toml")).unwrap(),
        before
    );
}

#[test]
fn kilo_pi_and_hermes_get_their_files_and_uninstall_takes_them_away() {
    let h = home("kilo-pi-hermes");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("integrations");
    let same = |a: &Path, b: &str| {
        assert_eq!(
            fs::read_to_string(a).unwrap(),
            fs::read_to_string(root.join(b)).unwrap(),
            "{}",
            a.display()
        )
    };
    for agent in ["kilo", "pi", "hermes"] {
        let o = dtk(&h, &h, &["init", "--agent", agent]);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{agent}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    // Kilo Code is a fork of OpenCode and runs the same plugin.
    same(&h.join(".config/kilo/plugin/dtk.ts"), "opencode/dtk.ts");
    same(&h.join(".pi/agent/extensions/dtk.ts"), "pi/dtk.ts");
    same(
        &h.join(".hermes/plugins/dtk/__init__.py"),
        "hermes/__init__.py",
    );
    same(
        &h.join(".hermes/plugins/dtk/plugin.yaml"),
        "hermes/plugin.yaml",
    );
    // Hermes loads a plugin only after it is enabled, and the message says how.
    let o = dtk(&h, &h, &["init", "--agent", "hermes"]);
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("hermes plugins enable dtk"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    for agent in ["kilo", "pi", "hermes"] {
        dtk(&h, &h, &["init", "--agent", agent, "--uninstall"]);
    }
    assert!(!h.join(".config/kilo/plugin/dtk.ts").exists());
    assert!(!h.join(".pi/agent/extensions/dtk.ts").exists());
    assert!(!h.join(".hermes/plugins/dtk").exists());
}

// ---- `dtk init --global`: Claude Code and every other agent that is there -----------------------------

/// A machine with Codex (and a memory tool's hooks), Gemini CLI (and its settings), OpenCode, Pi,
/// Hermes and Mistral Vibe (and a hook of its own). There is no Copilot CLI and no Kilo Code.
fn busy_machine(name: &str) -> (PathBuf, Vec<(PathBuf, String)>) {
    let h = home(name);
    let mut originals = Vec::new();
    let mut put = |rel: &str, text: String| {
        let p = h.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, &text).unwrap();
        originals.push((p, text));
    };
    put(
        ".codex/hooks.json",
        serde_json::to_string_pretty(&remem_codex_hooks()).unwrap(),
    );
    put(
        ".gemini/settings.json",
        json!({"theme": "dark", "hooks": {"BeforeTool": [{"matcher": "write_file",
            "hooks": [{"type": "command", "command": "./audit.sh"}]}]}})
        .to_string(),
    );
    put(
        ".vibe/hooks.toml",
        "[[hooks]]\nname = \"audit\"\ntype = \"pre_tool\"\ncommand = \"./audit.sh\"\n".into(),
    );
    fs::create_dir_all(h.join(".config/opencode")).unwrap();
    fs::create_dir_all(h.join(".pi/agent")).unwrap();
    fs::create_dir_all(h.join(".hermes")).unwrap();
    (h, originals)
}

#[test]
fn init_global_connects_every_agent_that_is_there_and_only_those() {
    let (h, originals) = busy_machine("global");
    let o = dtk(&h, &h, &["init", "--global"]);
    let said = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{said}");
    assert!(
        said.contains("other agents found: codex, gemini, vibe, opencode, pi, hermes"),
        "{said}"
    );
    // Claude Code, as it always was.
    assert!(fs::read_to_string(h.join(".claude/settings.json"))
        .unwrap()
        .contains("hook claude"));
    // Codex and Gemini keep what they had.
    let codex = json_of(&h.join(".codex/hooks.json"));
    assert_eq!(codex["hooks"]["Stop"], remem_codex_hooks()["hooks"]["Stop"]);
    assert!(codex["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("hook codex"));
    let gemini = json_of(&h.join(".gemini/settings.json"));
    assert_eq!(gemini["theme"], "dark");
    assert_eq!(
        gemini["hooks"]["AfterTool"][0]["matcher"],
        "run_shell_command|mcp_.*"
    );
    let vibe = fs::read_to_string(h.join(".vibe/hooks.toml")).unwrap();
    assert!(
        vibe.contains("name = \"audit\"") && vibe.contains("name = \"dtk\""),
        "{vibe}"
    );
    assert!(h.join(".config/opencode/plugins/dtk.ts").is_file());
    assert!(h.join(".pi/agent/extensions/dtk.ts").is_file());
    assert!(h.join(".hermes/plugins/dtk/__init__.py").is_file());
    // Not there, so nothing is made for them.
    assert!(!h.join(".copilot").exists(), "no Copilot CLI here");
    assert!(!h.join(".config/kilo").exists(), "no Kilo Code here");

    // Again: every file is as it was.
    let snapshot = |h: &Path| -> Vec<(String, String)> {
        let mut v = Vec::new();
        for rel in [
            ".codex/hooks.json",
            ".gemini/settings.json",
            ".vibe/hooks.toml",
            ".config/opencode/plugins/dtk.ts",
            ".pi/agent/extensions/dtk.ts",
            ".hermes/plugins/dtk/__init__.py",
            ".hermes/plugins/dtk/plugin.yaml",
            ".claude/settings.json",
        ] {
            v.push((rel.to_string(), fs::read_to_string(h.join(rel)).unwrap()));
        }
        v
    };
    let once = snapshot(&h);
    let o = dtk(&h, &h, &["init", "--global"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(snapshot(&h), once, "twice is once");

    // `doctor` sees dtk in each of them.
    let doctor = String::from_utf8_lossy(&dtk(&h, &h, &["doctor"]).stdout).to_string();
    for agent in ["codex", "gemini", "vibe", "opencode", "pi", "hermes"] {
        assert!(
            doctor.contains(&format!("{agent}: dtk is registered")),
            "{doctor}"
        );
    }

    // Out: every file that was there is as it was, and the files that dtk made are gone.
    let o = dtk(&h, &h, &["init", "--uninstall"]);
    assert_eq!(o.status.code(), Some(0));
    for (p, text) in &originals {
        let now: Value =
            serde_json::from_str(&fs::read_to_string(p).unwrap_or_default()).unwrap_or(Value::Null);
        let was: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        if was.is_null() {
            assert_eq!(&fs::read_to_string(p).unwrap(), text, "{}", p.display());
        } else {
            assert_eq!(now, was, "{}", p.display());
        }
    }
    assert!(!h.join(".config/opencode/plugins/dtk.ts").exists());
    assert!(!h.join(".pi/agent/extensions/dtk.ts").exists());
    assert!(!h.join(".hermes/plugins/dtk").exists());
}

#[test]
fn init_global_can_keep_to_claude_code() {
    let (h, _) = busy_machine("claude-only");
    let o = dtk(&h, &h, &["init", "--global", "--claude-only"]);
    let said = String::from_utf8_lossy(&o.stdout).to_string();
    assert_eq!(o.status.code(), Some(0), "{said}");
    assert!(h.join(".claude/settings.json").exists());
    assert!(!h.join(".config/opencode/plugins/dtk.ts").exists());
    assert!(!fs::read_to_string(h.join(".codex/hooks.json"))
        .unwrap()
        .contains("hook codex"));
    // It still says which agents have no dtk, and how to change that.
    assert!(
        said.contains(
            "codex is installed, and dtk is not registered with it. Run: dtk init --global"
        ),
        "{said}"
    );
}

#[test]
fn init_global_does_not_make_the_folder_of_claude_code_on_a_machine_that_has_only_other_agents() {
    let h = std::env::temp_dir().join(format!("dtk-agents-nocc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&h);
    fs::create_dir_all(h.join(".codex")).unwrap();
    let h = h.canonicalize().unwrap();
    let o = dtk(&h, &h, &["init", "--global"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        !h.join(".claude").exists(),
        "Claude Code is not here, so no folder is made for it"
    );
    assert!(fs::read_to_string(h.join(".codex/hooks.json"))
        .unwrap()
        .contains("hook codex"));
    // A machine with no agent at all: nothing is made, and it says what to do.
    let h2 = std::env::temp_dir().join(format!("dtk-agents-none-{}", std::process::id()));
    let _ = fs::remove_dir_all(&h2);
    fs::create_dir_all(&h2).unwrap();
    let h2 = h2.canonicalize().unwrap();
    let o = dtk(&h2, &h2, &["init", "--global"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(!h2.join(".claude").exists());
    assert!(String::from_utf8_lossy(&o.stdout).contains("no agent was found"));
}

#[test]
fn one_agent_that_cannot_be_changed_does_not_stop_the_others() {
    let (h, _) = busy_machine("broken");
    fs::write(h.join(".gemini/settings.json"), "{ not json").unwrap();
    let o = dtk(&h, &h, &["init", "--global"]);
    assert_eq!(o.status.code(), Some(1), "the broken file is reported");
    assert_eq!(
        fs::read_to_string(h.join(".gemini/settings.json")).unwrap(),
        "{ not json",
        "and not touched"
    );
    assert!(fs::read_to_string(h.join(".codex/hooks.json"))
        .unwrap()
        .contains("hook codex"));
    assert!(h.join(".pi/agent/extensions/dtk.ts").is_file());
    assert!(h.join(".claude/settings.json").exists());
}
