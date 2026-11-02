//! Gate G7: JSON documents that are not tables get an outline, but only when it helps.
//!
//! - A big document (or one that Claude Code saved to a file, of which the model would see only
//!   a preview) is replaced by an outline: every top-level key, the size of the parts, the shape
//!   of the records in each array, and a command to read any part.
//! - A small document is left alone. Its values are the point, and an outline would hide them.
//! - Tables are not outlined: they keep their row summaries.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-outline-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn run(home: &Path, args: &[&str], stdin: &str) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(args)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DTK_MODE")
        .env_remove("DTK_DISABLED")
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
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// A document like `terraform show -json`: a few top-level keys, one of them very big, with
/// records that have optional and nullable fields. Two arrays of objects, so it is not a table.
fn big_document() -> Value {
    let resources: Vec<Value> = (0..120)
        .map(|i| {
            let mut r = json!({
                "address": format!("aws_instance.web[{i}]"),
                "type": "aws_instance",
                "name": "web",
                "values": {"ami": "ami-0abc", "instance_type": "t3.micro", "tags": {"env": "prod"}, "root_block_device": [{"volume_size": 8 + i}]},
                "depends_on": ["aws_vpc.main"],
                "note": if i % 3 == 0 { Value::Null } else { json!("replaced") },
            });
            if i % 5 == 0 {
                r["tainted"] = json!(true);
            }
            r
        })
        .collect();
    let outputs: Vec<Value> = (0..40)
        .map(|i| json!({"name": format!("out{i}"), "sensitive": false, "value": format!("v{i}")}))
        .collect();
    json!({
        "format_version": "1.2",
        "terraform_version": "1.9.0",
        "planned_values": {"root_module": {"resources": resources}},
        "outputs": outputs,
        "applyable": true,
        "errored": false
    })
}

fn bash_event(stdout: &str, extra: Value) -> String {
    let mut resp = json!({"stdout": stdout, "stderr": "", "interrupted": false, "isImage": false});
    for (k, v) in extra.as_object().unwrap() {
        resp[k] = v.clone();
    }
    json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "terraform show -json plan"},
        "tool_response": resp,
    })
    .to_string()
}

fn stdout_of(hook_out: &str) -> Option<String> {
    let v: Value = serde_json::from_str(hook_out).ok()?;
    v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .map(String::from)
}

#[test]
fn a_big_document_becomes_an_outline_that_names_everything() {
    let home = temp_home("big");
    let doc = big_document().to_string();
    assert!(doc.len() > 20_000, "the fixture must be big: {}", doc.len());
    let (code, out) = run(&home, &["hook", "claude"], &bash_event(&doc, json!({})));
    assert_eq!(code, 0);
    let text = stdout_of(&out).unwrap_or_else(|| panic!("not replaced:\n{out}"));
    for want in [
        "json document",
        "an object with 6 keys",
        ".format_version: str \"1.2\"",
        ".terraform_version: str \"1.9.0\"",
        ".planned_values (",
        "resources:array[120] of {address:str, type:str, name:str",
        "tainted?:bool",
        "note:null|str",
        ".outputs (",
        "array[40] of {name:str, sensitive:bool, value:str}",
        ".applyable: bool true",
        ".errored: bool false",
        "drill: jaq",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    assert!(
        text.len() * 20 < doc.len(),
        "the outline must be small: {} vs {}",
        text.len(),
        doc.len()
    );
    // The full document is on disk, at the path the outline gives.
    let file = text
        .lines()
        .find_map(|l| l.strip_prefix("file: "))
        .expect(&text);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(saved, big_document());
}

#[test]
fn a_small_document_is_left_alone() {
    let home = temp_home("small");
    // About 3 KB: a package.json or a config. Its values are what the model reads it for.
    let doc = json!({
        "name": "app", "version": "1.2.3",
        "dependencies": (0..150).map(|i| (format!("dep{i}"), json!(format!("^{i}.0.0")))).collect::<serde_json::Map<_, _>>(),
    })
    .to_string();
    assert!(doc.len() > 2000 && doc.len() < 6000, "{}", doc.len());
    let (_, out) = run(&home, &["hook", "claude"], &bash_event(&doc, json!({})));
    assert!(
        out.trim().is_empty(),
        "a small document was replaced:\n{out}"
    );
    let (_, why) = run(&home, &["explain", "--command", "cat package.json"], &doc);
    assert!(
        why.contains("PASS THROUGH") && why.contains("values are the point"),
        "{why}"
    );
}

#[test]
fn a_document_saved_to_a_file_is_outlined_even_when_it_is_modest() {
    let home = temp_home("persisted");
    let doc = big_document().to_string();
    let saved = home.join(".claude/projects/p/tool-results/x.txt");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    std::fs::write(&saved, &doc).unwrap();
    let preview = &doc[..2000];
    let ev = bash_event(
        preview,
        json!({"persistedOutputPath": saved.display().to_string(), "persistedOutputSize": doc.len()}),
    );
    let (_, out) = run(&home, &["hook", "claude"], &ev);
    let text = stdout_of(&out).unwrap_or_else(|| panic!("not replaced:\n{out}"));
    assert!(text.contains("an object with 6 keys"), "{text}");
    let upd: Value = serde_json::from_str(&out).unwrap();
    assert!(
        upd["hookSpecificOutput"]["updatedToolOutput"]
            .get("persistedOutputPath")
            .is_none(),
        "the preview pointer must go: {out}"
    );
}

#[test]
fn tables_are_still_summarized_by_rows_not_outlined() {
    let home = temp_home("table");
    let rows: Vec<Value> = (0..300)
        .map(|i| json!({"id": i, "name": format!("n{i}"), "score": i * 3}))
        .collect();
    let doc = Value::Array(rows).to_string();
    let (_, out) = run(&home, &["hook", "claude"], &bash_event(&doc, json!({})));
    let text = stdout_of(&out).unwrap_or_else(|| panic!("not replaced:\n{out}"));
    assert!(text.contains("300 rows x 3 cols"), "{text}");
    assert!(!text.contains("json document"), "{text}");
}

#[test]
fn a_document_that_is_not_json_is_not_touched() {
    let home = temp_home("notjson");
    let broken = format!("{{\"a\": [{}", "1, ".repeat(3000));
    let (_, out) = run(&home, &["hook", "claude"], &bash_event(&broken, json!({})));
    assert!(out.trim().is_empty(), "{out}");
}

#[test]
fn explain_and_view_agree_on_a_document() {
    let home = temp_home("explain");
    let file = home.join("plan.json");
    std::fs::write(&file, big_document().to_string()).unwrap();
    let f = file.display().to_string();
    let (_, why) = run(&home, &["explain", &f], "");
    assert!(why.contains("detected:  json-document"), "{why}");
    assert!(why.contains("outline of the document"), "{why}");
    assert!(why.contains("decision:  APPLY"), "{why}");
    let (code, view) = run(&home, &["view", &f], "");
    assert_eq!(code, 0);
    assert!(view.contains("an object with 6 keys"), "{view}");
}

#[test]
fn what_the_model_asked_for_is_not_outlined() {
    let home = temp_home("asked");
    let doc = big_document().to_string();
    let store = home.join(".local/share/dtk/store/abc.json");
    for command in [
        // a jq query, plain or after rtk's rewrite
        "jq '.planned_values' plan.json",
        "rtk jq . plan.json",
        "cat plan.json | jaq -c '.outputs'",
        // the drill command that an outline prints
        &format!("jaq -c '.planned_values' {}", store.display()),
        &format!("cat {}", store.display()),
    ] {
        let (_, why) = run(&home, &["explain", "--command", command], &doc);
        assert!(
            why.contains("PASS THROUGH") && !why.contains("decision:  APPLY"),
            "`{command}` was outlined:\n{why}"
        );
    }
    // The same document, from a plain cat, is outlined.
    let (_, why) = run(&home, &["explain", "--command", "cat plan.json"], &doc);
    assert!(why.contains("decision:  APPLY"), "{why}");
}

#[test]
fn reading_a_file_needs_a_bigger_document_than_a_command_does() {
    let home = temp_home("read");
    // About 6,000 tokens: big enough for a command's output, not for a deliberate read.
    let doc = json!({
        "meta": {"v": 1},
        "a": (0..180).map(|i| json!({"k": format!("key{i}"), "v": "x".repeat(40), "n": i})).collect::<Vec<_>>(),
        "b": (0..180).map(|i| json!({"z": i, "tags": ["t1", "t2"], "w": "y".repeat(30)})).collect::<Vec<_>>(),
    })
    .to_string();
    let tokens = doc.len() / 4;
    assert!((4500..7500).contains(&tokens), "{tokens}");
    let (_, bash) = run(&home, &["explain", "--command", "cat doc.json"], &doc);
    assert!(bash.contains("decision:  APPLY"), "{bash}");
    let (_, read) = run(
        &home,
        &["explain", "--read", "--command", "/work/doc.json"],
        &doc,
    );
    assert!(read.contains("PASS THROUGH"), "{read}");
}

fn corpus_doc(name: &str) -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/documents")
            .join(name),
    )
    .unwrap()
}

#[test]
fn real_documents_get_outlines_that_keep_their_keys() {
    let home = temp_home("real");
    // `cargo metadata` of this repository (paths changed to /home/dev).
    let meta = corpus_doc("cargo_metadata.json");
    let (_, out) = run(&home, &["hook", "claude"], &bash_event(&meta, json!({})));
    let text = stdout_of(&out).unwrap_or_else(|| panic!("not replaced:\n{out}"));
    let root: Value = serde_json::from_str(&meta).unwrap();
    for key in root.as_object().unwrap().keys() {
        assert!(
            text.contains(&format!(".{key}")),
            "key {key} is missing:\n{text}"
        );
    }
    let n = root["packages"].as_array().unwrap().len();
    assert!(text.contains(&format!("array[{n}]")), "{text}");
    assert!(
        text.contains("version:int 1") || text.contains(".version: int 1"),
        "{text}"
    );
    assert!(
        text.len() * 40 < meta.len(),
        "{} vs {}",
        text.len(),
        meta.len()
    );

    // The first three releases of this repository from `gh api repos/amaye15/dtk/releases`:
    // a few big records, so no table.
    let rel = corpus_doc("gh_releases.json");
    let (_, out) = run(&home, &["hook", "claude"], &bash_event(&rel, json!({})));
    let text = stdout_of(&out).unwrap_or_else(|| panic!("not replaced:\n{out}"));
    assert!(text.contains("an array of 3"), "{text}");
    for tag in ["v0.4.0", "v0.3.0", "v0.2.0"] {
        assert!(text.contains(tag), "release {tag} is not named:\n{text}");
    }
    assert!(text.contains("tag_name:str"), "{text}");
}
