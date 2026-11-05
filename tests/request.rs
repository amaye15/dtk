//! Summaries that add the rows a request is about.
//!
//! The rule that makes this safe is the first test: with no word in the request, or with words
//! that match nothing, the summary is exactly what it was before. Everything else adds a row only
//! when a whole cell is one of the words of the request, in a column where that can mean a row.

use dtk::lean::{render, render_for, Ctx};
use dtk::table::detect;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ---- the library: lean::render_for ----------------------------------------------------------------

const NAMES: [&str; 12] = [
    "Ann", "Bob", "Cy", "Dee", "Eve", "Fay", "Gus", "Hal", "Ivy", "Jon", "Kay", "Lou",
];

/// 300 rows: `id` (all different), `name` (12 values, each a twelfth of the rows), `qty`
/// (whole numbers that repeat), `region` (3 values), `note` (`ok` in 3 rows of 5; a code in the
/// rest, each different), and `rare` (one name that is in only 2 rows).
fn orders() -> String {
    let mut s = String::from("id,name,qty,region,note,rare\n");
    for i in 0..300 {
        let note = if i % 5 < 3 {
            "ok".to_string()
        } else {
            format!("code{i:04}")
        };
        let rare = if i == 77 || i == 211 { "Zoltan" } else { "-" };
        s.push_str(&format!(
            "{},{},{},{},{},{}\n",
            1000 + i,
            NAMES[i % 12],
            1 + i % 9,
            ["us", "eu", "ap"][i % 3],
            note,
            rare
        ));
    }
    s
}

fn ctx() -> Ctx {
    Ctx {
        bytes: 9000,
        csv_path: Some("/tmp/x.csv".into()),
        raw_path: None,
    }
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|s| s.to_string()).collect()
}

fn view(keys: &[&str]) -> String {
    let t = detect(&orders()).unwrap();
    render_for(&t, &ctx(), 2500, &words(keys)).text
}

#[test]
fn with_no_word_or_no_match_the_summary_is_exactly_what_it_was() {
    let t = detect(&orders()).unwrap();
    let before = render(&t, &ctx(), 2500).text;
    assert!(!before.contains("matches for words"));
    assert_eq!(view(&[]), before);
    assert_eq!(view(&["zzzzqq", "nosuchname", "1234567"]), before);
}

#[test]
fn a_key_that_is_an_id_adds_that_row() {
    let s = view(&["1150"]);
    assert!(
        s.contains("matches for words in your request (id = 1150):"),
        "{s}"
    );
    assert!(s.lines().any(|l| l.starts_with("1150,Gus,")), "{s}");
}

#[test]
fn a_name_is_matched_as_a_whole_cell_and_the_case_does_not_matter() {
    let s = view(&["zoltan"]);
    assert!(s.contains("(rare = Zoltan)"), "{s}");
    // Both rows that have it.
    assert!(s.lines().any(|l| l.starts_with("1077,")), "{s}");
    assert!(s.lines().any(|l| l.starts_with("1211,")), "{s}");
    // A part of a cell is not the cell.
    assert!(!view(&["zolta"]).contains("matches for words"));
    assert!(!view(&["code00"]).contains("matches for words"));
}

#[test]
fn a_number_in_a_column_where_values_repeat_is_not_a_request_for_a_row() {
    // `qty` is 1 to 9, again and again; 3 is a quantity, not a row.
    assert!(!view(&["3"]).contains("matches for words"));
}

#[test]
fn a_word_that_matches_more_than_a_fifth_of_the_rows_names_nothing() {
    // `ok` is in 60% of the rows, `us` in a third, and each name in a twelfth (8%).
    assert!(!view(&["ok"]).contains("matches for words"));
    assert!(!view(&["us"]).contains("matches for words"));
    assert!(
        view(&["gus"]).contains("matches for words"),
        "a twelfth is not too many"
    );
}

#[test]
fn at_most_eight_rows_are_added() {
    // 25 rows have the name `Gus`; the summary adds 8 of them, and says what they have in common.
    let s = view(&["gus"]);
    let block = s
        .split("matches for words in your request (name = Gus):\n")
        .nth(1)
        .unwrap();
    let added = block
        .lines()
        .take_while(|l| !l.starts_with("drill:"))
        .count();
    assert!(added <= 8, "{added} rows in\n{s}");
}

#[test]
fn a_table_that_is_shown_whole_never_changes() {
    let small: String = orders().lines().take(41).collect::<Vec<_>>().join("\n");
    let t = detect(&small).unwrap();
    let plain = render(&t, &ctx(), 2500).text;
    let asked = render_for(&t, &ctx(), 2500, &words(&["1005"])).text;
    assert_eq!(plain, asked);
    assert!(plain.contains("nothing dropped"));
}

#[test]
fn a_row_that_the_summary_shows_already_is_not_added_again() {
    // The first row is in the head.
    let s = view(&["1000"]);
    assert!(!s.contains("matches for words"), "{s}");
}

// ---- the binary: the hook ---------------------------------------------------------------------------

const BIN: &str = env!("CARGO_BIN_EXE_dtk");

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-request-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude/projects/p")).unwrap();
    dir.canonicalize().unwrap()
}

fn hook(home: &Path, event: &Value) -> Option<String> {
    let mut child = Command::new(BIN)
        .args(["hook", "claude"])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DTK_MODE")
        .env_remove("DTK_DISABLED")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(
        v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()?
            .to_string(),
    )
}

fn event(command: &str, description: Option<&str>, transcript: Option<&Path>, text: &str) -> Value {
    let mut input = json!({"command": command});
    if let Some(d) = description {
        input["description"] = json!(d);
    }
    let mut ev = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash",
                        "tool_input": input,
                        "tool_response": {"stdout": text, "stderr": "", "interrupted": false, "isImage": false}});
    if let Some(t) = transcript {
        ev["transcript_path"] = json!(t.display().to_string());
    }
    ev
}

fn user_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

fn transcript(home: &Path, lines: &[String]) -> PathBuf {
    let p = home.join(".claude/projects/p/session.jsonl");
    fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

fn has_block(summary: &str, label: &str) -> bool {
    summary.contains(&format!("matches for words in your request ({label})"))
}

#[test]
fn the_description_of_the_tool_call_carries_the_request() {
    let h = home("description");
    let with = hook(
        &h,
        &event(
            "cat orders.csv",
            Some("Look up order 1150"),
            None,
            &orders(),
        ),
    )
    .unwrap();
    assert!(has_block(&with, "id = 1150"), "{with}");
    let without = hook(
        &h,
        &event("cat orders.csv", Some("Show the orders"), None, &orders()),
    )
    .unwrap();
    assert!(!without.contains("matches for words"), "{without}");
}

#[test]
fn what_the_user_typed_is_read_from_the_end_of_the_transcript() {
    let h = home("transcript");
    // The request, then more than a window's worth of tool output after it.
    let filler = json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t", "content": "x".repeat(400_000)}]}})
    .to_string();
    let t = transcript(
        &h,
        &[
            user_line("an old question about Zoltan"),
            user_line("what is the region of order 1150?"),
            filler,
        ],
    );
    let s = hook(&h, &event("cat orders.csv", None, Some(&t), &orders())).unwrap();
    assert!(has_block(&s, "id = 1150"), "{s}");
    assert!(
        !s.contains("rare = Zoltan"),
        "the old question was used:\n{s}"
    );
}

#[test]
fn the_last_prompt_entry_is_not_trusted_because_it_lags() {
    let h = home("lastprompt");
    let t = transcript(
        &h,
        &[
            user_line("what is the region of order 1150?"),
            // Claude Code wrote this entry for the previous prompt, after the new one began.
            json!({"type": "last-prompt", "lastPrompt": "look up Zoltan"}).to_string(),
        ],
    );
    let s = hook(&h, &event("cat orders.csv", None, Some(&t), &orders())).unwrap();
    assert!(has_block(&s, "id = 1150"), "{s}");
    // The column statistics name Zoltan; only a match block would show the row.
    assert!(!s.contains("(rare = Zoltan)"), "{s}");
}

#[test]
fn a_transcript_outside_the_claude_folder_is_not_read() {
    let h = home("outside");
    let elsewhere = h.join("elsewhere.jsonl");
    fs::write(
        &elsewhere,
        user_line("what is the region of order 1150?") + "\n",
    )
    .unwrap();
    let s = hook(
        &h,
        &event("cat orders.csv", None, Some(&elsewhere), &orders()),
    )
    .unwrap();
    assert!(!s.contains("matches for words"), "{s}");
}

#[test]
fn a_missing_or_broken_transcript_changes_nothing() {
    let h = home("broken");
    let plain = hook(&h, &event("cat orders.csv", None, None, &orders())).unwrap();
    let gone = h.join(".claude/projects/p/gone.jsonl");
    assert_eq!(
        hook(&h, &event("cat orders.csv", None, Some(&gone), &orders())).unwrap(),
        plain
    );
    let t = transcript(
        &h,
        &["not json".to_string(), "{\"type\":\"user\"".to_string()],
    );
    assert_eq!(
        hook(&h, &event("cat orders.csv", None, Some(&t), &orders())).unwrap(),
        plain
    );
}

#[test]
fn the_setting_turns_it_off() {
    let h = home("off");
    fs::create_dir_all(h.join(".config/dtk")).unwrap();
    fs::write(
        h.join(".config/dtk/config.toml"),
        "[hooks]\nrequest_rows = false\n",
    )
    .unwrap();
    let s = hook(
        &h,
        &event(
            "cat orders.csv",
            Some("Look up order 1150"),
            None,
            &orders(),
        ),
    )
    .unwrap();
    assert!(!s.contains("matches for words"), "{s}");
}

// ---- the corpus: no output changes unless a word of the request is in it --------------------------

#[test]
fn no_fixture_of_the_corpus_changes_for_a_request_that_matches_nothing() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let h = home("corpus");
    let (mut compared, mut summarized) = (0, 0);
    for e in manifest["positive"].as_array().unwrap() {
        let file = e["file"].as_str().unwrap();
        let text = String::from_utf8_lossy(&fs::read(dir.join(file)).unwrap()).to_string();
        let plain = hook(&h, &event("mytool --run", None, None, &text));
        let asked = hook(
            &h,
            &event(
                "mytool --run",
                Some("Check zzyzxq and qqqzzy for xjxjxj"),
                None,
                &text,
            ),
        );
        // The path of a saved file is new each time; the rest must be the same.
        let norm = |s: Option<String>| {
            s.map(|s| {
                s.lines()
                    .filter(|l| {
                        !l.starts_with("data: ")
                            && !l.starts_with("raw: ")
                            && !l.starts_with("drill: ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        };
        assert_eq!(norm(plain.clone()), norm(asked), "{file}");
        compared += 1;
        summarized += usize::from(plain.is_some());
    }
    assert!(
        compared >= 80 && summarized >= 40,
        "{compared} compared, {summarized} summarized"
    );
}
