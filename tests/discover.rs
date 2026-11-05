//! `dtk discover --gaps` lists the big outputs that dtk left alone and that are data-like; `--dump`
//! writes the big outputs as files, for `scripts/compare-builds.sh`.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-discover-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude/projects/p")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .arg("discover")
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

/// A transcript with one Bash call and its result for each (command, output).
fn transcript(home: &Path, calls: &[(&str, String)]) {
    let mut lines = Vec::new();
    for (i, (command, output)) in calls.iter().enumerate() {
        let id = format!("toolu_{i}");
        lines.push(
            json!({"message": {"content": [{"type": "tool_use", "id": id, "name": "Bash",
                                            "input": {"command": command}}]}})
            .to_string(),
        );
        lines.push(
            json!({"message": {"content": [{"type": "tool_result", "tool_use_id": id,
                                            "content": output}]}})
            .to_string(),
        );
    }
    fs::write(
        home.join(".claude/projects/p/session.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

/// A listing of tensors: name, type and shape in lines of different widths. No reader of dtk takes
/// it for a table today. If one does, use another example here.
fn tensors() -> String {
    (0..160)
        .map(|i| {
            format!(
                "  icl_blocks.{}.attention.k_projection.weight{} F16 [512, {}]\n",
                i % 24,
                " ".repeat(i % 7),
                64 * (1 + i % 3)
            )
        })
        .collect()
}

fn csv() -> String {
    let mut s = String::from("id,name,region,amount\n");
    for i in 0..300 {
        s.push_str(&format!(
            "{i},user_{},{},{}.5\n",
            i % 9,
            ["us", "eu", "ap"][i % 3],
            i * 3
        ));
    }
    s
}

fn source() -> String {
    "fn main() {\n    let a = f(1, 2, 3);\n    println!(\"{}\", g(a, b));\n}\n".repeat(120)
}

fn prose() -> String {
    "# Notes\n\nThis section explains how the build works, and why the steps come in this order.\n\n"
        .repeat(60)
}

#[test]
fn gaps_lists_data_that_dtk_did_not_read_and_not_source_prose_or_tables() {
    let h = home("gaps");
    transcript(
        &h,
        &[
            ("python dump_tensors.py", tensors()),
            ("cat src/main.rs", source()),
            ("cat data.csv", csv()),
            ("cat NOTES.md", prose()),
            ("echo done", "done\n".into()),
        ],
    );
    let (code, out) = dtk(&h, &["--gaps", "--json"]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["gaps"], 1, "only the tensor listing is a gap:\n{out}");
    assert_eq!(v["by_command"][0]["command"], "python", "{out}");
    let first = &v["biggest"][0];
    assert!(
        first["first_line"].as_str().unwrap().contains("icl_blocks"),
        "{out}"
    );
    assert_eq!(first["lines"], 160);
    // The plain text report says the same.
    let (_, text) = dtk(&h, &["--gaps"]);
    assert!(text.contains("1 big outputs"), "{text}");
    assert!(text.contains("python"), "{text}");
}

#[test]
fn the_same_output_twice_is_one_gap_with_a_count() {
    let h = home("twice");
    transcript(
        &h,
        &[
            ("python dump_tensors.py", tensors()),
            ("python dump_tensors.py", tensors()),
        ],
    );
    let (_, text) = dtk(&h, &["--gaps"]);
    assert!(text.contains("2 big outputs"), "{text}");
    assert!(text.contains("x2"), "{text}");
}

#[test]
fn dump_writes_each_big_output_as_a_file() {
    let h = home("dump");
    transcript(
        &h,
        &[
            ("python dump_tensors.py", tensors()),
            ("cat data.csv", csv()),
            ("echo done", "done\n".into()),
        ],
    );
    let dir = h.join("dumped");
    let (code, _) = dtk(&h, &["--dump", dir.to_str().unwrap()]);
    assert_eq!(code, 0);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["h00000.txt", "h00001.txt"],
        "the small output is not written"
    );
    assert_eq!(
        fs::read_to_string(dir.join("h00000.txt")).unwrap(),
        tensors()
    );
}

#[test]
fn a_bad_option_is_refused() {
    let h = home("bad");
    let (code, _) = dtk(&h, &["--dump"]);
    assert_eq!(code, 2);
    let (code, _) = dtk(&h, &["--nonsense"]);
    assert_eq!(code, 2);
}
