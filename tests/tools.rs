//! S5: explain, verify, discover, config and gain --json.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-tools-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn run(home: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dtk"));
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

fn csv(n: usize) -> String {
    let mut s = String::from("id,name,region,amount\n");
    for i in 0..n {
        s.push_str(&format!(
            "{},user_{},{},{}.5\n",
            1000 + i,
            i % 7,
            ["us", "eu", "ap"][i % 3],
            i * 3
        ));
    }
    s
}

// ---- explain ---------------------------------------------------------------------------------

#[test]
fn explain_says_apply_and_why() {
    let home = temp_home("explain");
    let (code, out, _) = run(
        &home,
        &["explain", "-", "--command", "cat events.jsonl"],
        &jsonl(300),
        &[],
    );
    assert_eq!(code, 0);
    assert!(out.contains("detected:  jsonl, 300 rows x 4 cols"), "{out}");
    assert!(out.contains("decision:  APPLY (the summary is"), "{out}");
    assert!(out.contains("mode:      live"), "{out}");
}

#[test]
fn explain_gives_a_reason_for_every_pass_through() {
    let home = temp_home("explainreasons");
    let case = |args: &[&str], stdin: &str, env: &[(&str, &str)], want: &str| {
        let (_, out, _) = run(&home, args, stdin, env);
        assert!(
            out.contains("PASS THROUGH") && out.contains(want),
            "wanted {want:?} in:\n{out}"
        );
    };
    case(
        &["explain", "-", "--command", "head -n 300 events.jsonl"],
        &jsonl(300),
        &[],
        "asks for exact rows",
    );
    case(
        &["explain", "-", "--command", "cat notes.txt"],
        &"plain words and more words\n".repeat(100),
        &[],
        "not a table",
    );
    case(
        &["explain", "-", "--command", "cat small.jsonl"],
        &jsonl(12),
        &[],
        "small output",
    );
    case(
        &["explain", "-", "--command", "cat events.jsonl"],
        &jsonl(300),
        &[("DTK_DISABLED", "1")],
        "DTK_DISABLED=1",
    );
    case(
        &["explain", "-", "--read", "--command", "/w/main.rs"],
        &jsonl(300),
        &[],
        "not a data file type",
    );
    // Shadow mode: would apply, but nothing changes.
    let (_, out, _) = run(
        &home,
        &["explain", "-", "--command", "cat e.jsonl"],
        &jsonl(300),
        &[("DTK_MODE", "shadow")],
    );
    assert!(
        out.contains("WOULD APPLY in live mode") && out.contains("shadow"),
        "{out}"
    );
    // Config exclusion.
    fs::create_dir_all(home.join(".config/dtk")).unwrap();
    fs::write(
        home.join(".config/dtk/config.toml"),
        "[hooks]\nexclude_commands = [\"^cat \"]\n",
    )
    .unwrap();
    case(
        &["explain", "-", "--command", "cat events.jsonl"],
        &jsonl(300),
        &[],
        "exclude_commands",
    );
}

fn bash_event(command: &str, stdout: &str) -> String {
    json!({
        "hook_event_name": "PostToolUse", "tool_name": "Bash",
        "tool_input": {"command": command},
        "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false, "isImage": false},
    })
    .to_string()
}

/// The point of sharing one decision function: explain never disagrees with the hook.
#[test]
fn explain_agrees_with_the_hook_on_every_corpus_fixture() {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(corpus.join("manifest.json")).unwrap()).unwrap();
    let home = temp_home("agree");
    let mut checked = 0;
    for kind in ["positive", "negative", "either"] {
        for e in manifest[kind].as_array().unwrap() {
            let file = e["file"].as_str().unwrap();
            let text = String::from_utf8_lossy(&fs::read(corpus.join(file)).unwrap()).to_string();
            let command = "cat data";
            let (_, explained, _) = run(&home, &["explain", "-", "--command", command], &text, &[]);
            let explain_applies = explained.contains("decision:  APPLY");
            let (code, hooked, _) =
                run(&home, &["hook", "claude"], &bash_event(command, &text), &[]);
            assert_eq!(code, 0);
            assert_eq!(
                !hooked.is_empty(),
                explain_applies,
                "{file}: hook output empty={}, explain said:\n{explained}",
                hooked.is_empty()
            );
            checked += 1;
        }
    }
    assert!(checked >= 60, "only {checked} fixtures");
}

// ---- verify ----------------------------------------------------------------------------------

#[test]
fn verify_passes_its_built_in_checks() {
    let home = temp_home("verify");
    let (_, out, _) = run(&home, &["verify"], "", &[]);
    assert!(!out.contains("FAIL"), "{out}");
    let pass = out.lines().filter(|l| l.starts_with("PASS")).count();
    assert!(pass >= 20, "only {pass} checks ran:\n{out}");
    assert!(
        out.contains(&format!("{pass} of {pass} built-in checks passed")),
        "{out}"
    );
    for s in [
        "JSON lines",
        "CSV",
        "logfmt",
        "web access log",
        "polars table",
        "JSON object wrapping an array",
        "prefixed JSON lines",
    ] {
        assert!(
            out.contains(&format!("PASS  {s}: the view is under a third")),
            "{s}:\n{out}"
        );
    }
    assert!(out.contains("setup:"), "{out}");
}

// ---- discover --------------------------------------------------------------------------------

fn transcript_lines(home: &Path) -> String {
    let saved_dir = home.join(".claude/projects/proj/session/tool-results");
    fs::create_dir_all(&saved_dir).unwrap();
    let saved = saved_dir.join("bsaved1");
    fs::write(&saved, jsonl(2000)).unwrap();
    let use_line = |id: &str, name: &str, input: Value| {
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}).to_string()
    };
    let result_line = |id: &str, content: &str| {
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": content}]}}).to_string()
    };
    let numbered: String = csv(300)
        .lines()
        .enumerate()
        .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
        .collect();
    [
        // 1. Bash output shown in full: a 300-row JSONL table.
        use_line("t1", "Bash", json!({"command": "cat events.jsonl"})),
        result_line("t1", &jsonl(300)),
        // 2. Bash output saved to a file by Claude Code (2,000 rows on disk).
        use_line("t2", "Bash", json!({"command": "cat big.jsonl"})),
        result_line("t2", &format!("<persisted-output>\nOutput too large (140KB). Full output saved to: {}\n\nPreview (first 2KB):\n{{\"x\":1}}\n...\n</persisted-output>", saved.display())),
        // 3. Read of a CSV file (with line numbers).
        use_line("t3", "Read", json!({"file_path": "/w/orders.csv"})),
        result_line("t3", &numbered),
        // 4. A ranged Read of the same file: exact lines were asked for, so it is skipped.
        use_line("t4", "Read", json!({"file_path": "/w/orders.csv", "offset": 10, "limit": 300})),
        result_line("t4", &numbered),
        // 5. Big, but not a table.
        use_line("t5", "Bash", json!({"command": "cat notes.txt"})),
        result_line("t5", &"some prose about the project and how it works\n".repeat(200)),
        // 6. A table, but the command asks for exact rows.
        use_line("t6", "Bash", json!({"command": "head -n 300 events.jsonl"})),
        result_line("t6", &jsonl(300)),
    ]
    .join("\n")
        + "\n"
}

#[test]
fn discover_counts_exactly_what_a_hand_built_transcript_holds() {
    let home = temp_home("discover");
    let dir = home.join(".claude/projects/proj");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.jsonl"), transcript_lines(&home)).unwrap();
    let (code, out, _) = run(&home, &["discover", "--all", "--json"], "", &[]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["sessions"], 1);
    assert_eq!(v["tool_results"], 6);
    assert_eq!(
        v["tables"], 3,
        "events.jsonl, big.jsonl and orders.csv: {v}"
    );
    assert_eq!(v["would_replace"], 3);
    assert_eq!(
        v["shown_in_full"]["outputs"], 2,
        "the inline JSONL and the CSV read"
    );
    assert_eq!(v["saved_to_file"]["outputs"], 1);
    assert!(
        v["shown_in_full"]["est_tokens_saved"].as_i64().unwrap() > 3000,
        "{v}"
    );
    assert_eq!(v["by_format"]["jsonl"], 2);
    assert_eq!(v["by_format"]["csv"], 1);
    // The plain-text report names the same numbers.
    let (_, text, _) = run(&home, &["discover", "--all"], "", &[]);
    assert!(
        text.contains("tables found: 3") && text.contains("dtk would replace 3 of them"),
        "{text}"
    );
}

#[test]
fn discover_on_an_empty_history_is_quiet_and_exact() {
    let home = temp_home("discoverempty");
    let (code, out, _) = run(&home, &["discover"], "", &[]);
    assert_eq!(code, 0);
    assert!(
        out.contains("0 sessions") && out.contains("tables found: 0"),
        "{out}"
    );
}

// ---- config ----------------------------------------------------------------------------------

#[test]
fn config_create_show_and_a_broken_file() {
    let home = temp_home("config");
    let (code, out, _) = run(&home, &["config", "--create"], "", &[]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        run(&home, &["config", "--create"], "", &[]).0,
        1,
        "never overwrites"
    );
    let (_, out, _) = run(&home, &["config"], "", &[]);
    assert!(
        out.contains("min_tokens:         1200") && out.contains("exclude_commands:   []"),
        "{out}"
    );
    // A typo must not break the hook: defaults are used, and doctor says so.
    fs::write(home.join(".config/dtk/config.toml"), "[hooks\nbroken").unwrap();
    let (_, out, _) = run(
        &home,
        &["hook", "claude"],
        &bash_event("cat e.jsonl", &jsonl(300)),
        &[],
    );
    assert!(
        out.contains("[dtk] jsonl"),
        "defaults still summarize: {out}"
    );
    let (code, out, _) = run(&home, &["doctor"], "", &[]);
    assert!(out.contains("[warn] config.toml is ignored"), "{out}");
    assert_eq!(code, 1, "doctor exits 1 on a warning");
}

#[test]
fn thresholds_and_exclusions_change_the_hook() {
    let home = temp_home("thresholds");
    fs::create_dir_all(home.join(".config/dtk")).unwrap();
    let ev = bash_event("cat events.jsonl", &jsonl(300));
    fs::write(
        home.join(".config/dtk/config.toml"),
        "[thresholds]\nmin_tokens = 1000000\n",
    )
    .unwrap();
    assert_eq!(
        run(&home, &["hook", "claude"], &ev, &[]).1,
        "",
        "min_tokens above the output size"
    );
    fs::write(
        home.join(".config/dtk/config.toml"),
        "[thresholds]\nmin_tokens = 100\n",
    )
    .unwrap();
    assert!(run(&home, &["hook", "claude"], &ev, &[])
        .1
        .contains("[dtk]"));
    fs::write(
        home.join(".config/dtk/config.toml"),
        "[hooks]\nexclude_commands = [\"events.jsonl\"]\n",
    )
    .unwrap();
    assert_eq!(
        run(&home, &["hook", "claude"], &ev, &[]).1,
        "",
        "excluded by a substring"
    );
}

// ---- gain --json -----------------------------------------------------------------------------

#[test]
fn gain_json_is_valid_and_counts_decisions() {
    let home = temp_home("gainjson");
    let ev = bash_event("cat events.jsonl", &jsonl(300));
    run(&home, &["hook", "claude"], &ev, &[]);
    run(&home, &["hook", "claude"], &ev, &[("DTK_MODE", "shadow")]);
    let (code, out, _) = run(&home, &["gain", "--json"], "", &[]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"));
    assert_eq!(v["tables_seen"], 2);
    assert_eq!(v["shown_in_full"]["outputs"], 1);
    assert_eq!(v["shadow_only"]["outputs"], 1);
    assert_eq!(v["by_format"]["jsonl"], 2);
    assert!(v["shown_in_full"]["est_tokens_saved"].as_i64().unwrap() > 0);
    assert!(v["rtk"].is_null(), "rtk is not on this PATH");
}
