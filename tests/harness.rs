//! dtk's hook for the agents other than Claude Code.
//!
//! The payloads are written as the documentation of each agent says (Codex CLI hooks, Gemini CLI
//! hooks, Copilot CLI hooks reference, and OpenCode's plugin API as its own code calls it). They
//! are not the output of those programs, except for OpenCode (see `scripts/opencode-test.sh`).
//! What is checked is that each reply has the shape that the agent documents, that the same rules
//! as for Claude Code decide, and that everything dtk should not touch is left alone.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_dtk");

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-harness-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

/// `dtk hook AGENT` with `input` on standard input: (exit code, the reply if there was one).
fn hook(home: &Path, agent: &str, input: &str, env: &[(&str, &str)]) -> (i32, Option<Value>) {
    let mut cmd = Command::new(BIN);
    cmd.args(["hook", agent])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DTK_MODE")
        .env_remove("DTK_DISABLED");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let reply = serde_json::from_slice(&out.stdout).ok();
    (out.status.code().unwrap_or(-1), reply)
}

fn orders() -> String {
    let mut s = String::from("order_id,customer,region,amount,status\n");
    for i in 0..300 {
        s.push_str(&format!(
            "{},cust_{:03},{},{}.5,{}\n",
            1000 + i,
            i % 40,
            ["us", "eu", "ap"][i % 3],
            i * 3,
            ["ok", "ok", "failed"][i % 3]
        ));
    }
    s
}

fn rows_json() -> String {
    let rows: Vec<Value> = (0..300)
        .map(|i| {
            let region = ["us", "eu", "ap"][i % 3];
            json!({"id": 1000 + i, "name": format!("user_{}", i % 40), "amount": i * 3, "region": region})
        })
        .collect();
    serde_json::to_string(&rows).unwrap()
}

fn assert_summary(text: &str) {
    assert!(
        text.starts_with("[dtk] "),
        "{}",
        &text[..text.len().min(200)]
    );
    assert!(
        text.len() * 4 < orders().len().max(rows_json().len()),
        "{} bytes",
        text.len()
    );
}

// ---- Codex CLI: PostToolUse, `{"decision":"block","reason":TEXT}` ------------------------------

fn codex(tool: &str, input: Value, response: Value) -> String {
    json!({"session_id": "s1", "transcript_path": "/tmp/rollout.jsonl", "cwd": "/tmp",
           "hook_event_name": "PostToolUse", "model": "gpt-5", "permission_mode": "default",
           "turn_id": "t1", "tool_name": tool, "tool_use_id": "call_1",
           "tool_input": input, "tool_response": response})
    .to_string()
}

#[test]
fn codex_gets_the_summary_as_the_feedback_that_replaces_the_result() {
    let h = home("codex");
    for response in [json!(orders()), json!({"output": orders(), "exit_code": 0})] {
        let (code, reply) = hook(
            &h,
            "codex",
            &codex("Bash", json!({"command": "cat orders.csv"}), response),
            &[],
        );
        assert_eq!(code, 0);
        let r = reply.expect("a reply");
        assert_eq!(r["decision"], "block");
        assert_summary(r["reason"].as_str().unwrap());
    }
    // An MCP tool, named as Codex names them.
    let (_, reply) = hook(
        &h,
        "codex",
        &codex("mcp__db__query", json!({}), json!(rows_json())),
        &[],
    );
    let r = reply.expect("a reply for the MCP result");
    assert_eq!(r["decision"], "block");
    assert!(r["reason"].as_str().unwrap().starts_with("[dtk] "));
}

#[test]
fn codex_leaves_alone_what_dtk_leaves_alone() {
    let h = home("codex-alone");
    let ask = |tool: &str, command: &str, response: Value| {
        hook(
            &h,
            "codex",
            &codex(tool, json!({"command": command}), response),
            &[],
        )
        .1
    };
    assert!(
        ask("Bash", "cat small.csv", json!("a,b\n1,2\n")).is_none(),
        "small output"
    );
    assert!(
        ask("Bash", "head -n 100 orders.csv", json!(orders())).is_none(),
        "head asks for rows"
    );
    assert!(
        ask("apply_patch", "x", json!(orders())).is_none(),
        "another tool"
    );
    assert!(
        ask("Bash", "echo hi", json!("just words\n")).is_none(),
        "not data"
    );
    let (code, reply) = hook(&h, "codex", "this is not json", &[]);
    assert_eq!((code, reply), (0, None));
    let (_, reply) = hook(
        &h,
        "codex",
        &codex(
            "Bash",
            json!({"command": "cat orders.csv"}),
            json!(orders()),
        ),
        &[("DTK_DISABLED", "1")],
    );
    assert!(reply.is_none(), "DTK_DISABLED");
    // Another event is not for dtk.
    let pre = codex(
        "Bash",
        json!({"command": "cat orders.csv"}),
        json!(orders()),
    )
    .replace("PostToolUse", "PreToolUse");
    assert!(hook(&h, "codex", &pre, &[]).1.is_none());
}

#[test]
fn codex_never_replaces_the_answer_to_a_dtk_query() {
    let h = home("codex-query");
    let big = (0..200)
        .map(|i| format!("row {i}: {i},user_{i},{}.5\n", i * 3))
        .collect::<String>();
    let command = format!("{BIN} rows abcdef012345 0 200");
    let (_, reply) = hook(
        &h,
        "codex",
        &codex("Bash", json!({"command": command}), json!(big)),
        &[],
    );
    assert!(reply.is_none(), "the rows were asked for");
}

// ---- Gemini CLI: AfterTool, `{"decision":"deny","reason":TEXT}` ----------------------------------

fn gemini(tool: &str, input: Value, llm: Value) -> String {
    json!({"session_id": "s1", "transcript_path": "/tmp/chat.json", "cwd": "/tmp",
           "hook_event_name": "AfterTool", "timestamp": "2026-11-09T00:00:00Z",
           "tool_name": tool, "tool_input": input,
           "tool_response": {"llmContent": llm, "returnDisplay": "shown to the user"}})
    .to_string()
}

#[test]
fn gemini_keeps_its_own_lines_around_the_output_it_replaces() {
    let h = home("gemini");
    let llm = format!(
        "Command: cat orders.csv\nDirectory: (root)\nOutput: {}\nError: (none)\nExit Code: 0\nSignal: (none)\nBackground PIDs: (none)\nProcess Group PGID: 4242",
        orders().trim_end()
    );
    let (_, reply) = hook(
        &h,
        "gemini",
        &gemini(
            "run_shell_command",
            json!({"command": "cat orders.csv"}),
            json!(llm),
        ),
        &[],
    );
    let r = reply.expect("a reply");
    assert_eq!(r["decision"], "deny");
    let reason = r["reason"].as_str().unwrap();
    assert!(
        reason
            .starts_with("Command: cat orders.csv\nDirectory: (root)\nOutput: [dtk] csv: 300 rows"),
        "{}",
        &reason[..reason.len().min(200)]
    );
    assert!(reason.ends_with("\nError: (none)\nExit Code: 0\nSignal: (none)\nBackground PIDs: (none)\nProcess Group PGID: 4242"), "{}", &reason[reason.len().saturating_sub(150)..]);
    assert!(reason.len() * 4 < orders().len());
}

#[test]
fn gemini_text_without_its_lines_and_mcp_results_are_replaced_whole() {
    let h = home("gemini-plain");
    let (_, reply) = hook(
        &h,
        "gemini",
        &gemini(
            "run_shell_command",
            json!({"command": "cat orders.csv"}),
            json!(orders()),
        ),
        &[],
    );
    assert_summary(reply.expect("a reply")["reason"].as_str().unwrap());
    // An MCP result is a list of parts.
    let parts = json!([{"text": rows_json()}]);
    let (_, reply) = hook(&h, "gemini", &gemini("mcp_db_query", json!({}), parts), &[]);
    let r = reply.expect("a reply for the MCP result");
    assert_eq!(r["decision"], "deny");
    assert!(r["reason"].as_str().unwrap().starts_with("[dtk] "));
    // What is not for dtk.
    assert!(hook(
        &h,
        "gemini",
        &gemini("read_file", json!({}), json!(orders())),
        &[]
    )
    .1
    .is_none());
    assert!(hook(
        &h,
        "gemini",
        &gemini(
            "run_shell_command",
            json!({"command": "echo hi"}),
            json!("Command: echo hi\nOutput: hi\nError: (none)")
        ),
        &[]
    )
    .1
    .is_none());
    let before = gemini(
        "run_shell_command",
        json!({"command": "cat orders.csv"}),
        json!(orders()),
    )
    .replace("AfterTool", "BeforeTool");
    assert!(hook(&h, "gemini", &before, &[]).1.is_none());
}

// ---- Copilot CLI: postToolUse, `{"modifiedResult":{...}}` ---------------------------------------

fn copilot(args: Value, kind: &str, text: &str) -> String {
    json!({"sessionId": "s1", "timestamp": 1_700_000_000, "cwd": "/tmp", "toolName": "bash",
           "toolArgs": args, "toolResult": {"resultType": kind, "textResultForLlm": text}})
    .to_string()
}

#[test]
fn copilot_gets_a_modified_result() {
    let h = home("copilot");
    let (_, reply) = hook(
        &h,
        "copilot",
        &copilot(json!({"command": "cat orders.csv"}), "success", &orders()),
        &[],
    );
    let r = reply.expect("a reply");
    assert_eq!(r["modifiedResult"]["resultType"], "success");
    assert_summary(r["modifiedResult"]["textResultForLlm"].as_str().unwrap());
    // The arguments may come as a string of JSON.
    let (_, reply) = hook(
        &h,
        "copilot",
        &copilot(
            json!("{\"command\":\"cat orders.csv\"}"),
            "success",
            &orders(),
        ),
        &[],
    );
    assert!(reply.is_some());
    // The names that VS Code uses.
    let pascal = json!({"tool_name": "bash", "tool_input": {"command": "cat orders.csv"},
                        "tool_result": {"result_type": "success", "text_result_for_llm": orders()}})
    .to_string();
    let (_, reply) = hook(&h, "copilot", &pascal, &[]);
    let r = reply.expect("a reply in the other names");
    assert_summary(r["modifiedResult"]["text_result_for_llm"].as_str().unwrap());
    assert_eq!(r["modifiedResult"]["result_type"], "success");
}

#[test]
fn copilot_leaves_a_failure_another_tool_and_small_output_alone() {
    let h = home("copilot-alone");
    assert!(hook(
        &h,
        "copilot",
        &copilot(json!({"command": "cat orders.csv"}), "failure", &orders()),
        &[]
    )
    .1
    .is_none());
    assert!(hook(
        &h,
        "copilot",
        &copilot(json!({"command": "cat s.csv"}), "success", "a,b\n1,2\n"),
        &[]
    )
    .1
    .is_none());
    let view = json!({"toolName": "view", "toolArgs": {"path": "orders.csv"},
                      "toolResult": {"resultType": "success", "textResultForLlm": orders()}})
    .to_string();
    assert!(hook(&h, "copilot", &view, &[]).1.is_none());
}

// ---- OpenCode: the plugin calls `dtk hook opencode` ------------------------------------------------

fn opencode(tool: &str, args: Value, output: Value) -> String {
    json!({"tool": tool, "args": args, "output": output}).to_string()
}

#[test]
fn opencode_gets_the_text_for_the_output_and_the_words_of_the_description() {
    let h = home("opencode");
    let out = json!({"title": "cat orders.csv", "metadata": {"exit": 0}, "output": orders()});
    let (_, reply) = hook(
        &h,
        "opencode",
        &opencode("bash", json!({"command": "cat orders.csv"}), out.clone()),
        &[],
    );
    assert_summary(reply.expect("a reply")["output"].as_str().unwrap());
    // The description that the model wrote for the call is a request: it names the row.
    let args = json!({"command": "cat orders.csv", "description": "Look up order 1150"});
    let (_, reply) = hook(&h, "opencode", &opencode("bash", args, out), &[]);
    assert!(reply.unwrap()["output"]
        .as_str()
        .unwrap()
        .contains("matches for words in your request (order_id = 1150)"));
}

#[test]
fn opencode_mcp_results_keep_their_shape_and_images_are_left_alone() {
    let h = home("opencode-mcp");
    let blocks = json!({"content": [{"type": "text", "text": rows_json()}], "isError": false});
    let (_, reply) = hook(
        &h,
        "opencode",
        &opencode("db_query", json!({}), blocks),
        &[],
    );
    let r = reply.expect("a reply");
    assert_eq!(r["content"][0]["type"], "text");
    assert!(r["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("[dtk] "));
    let with_image = json!({"content": [{"type": "text", "text": rows_json()}, {"type": "image", "data": "AAAA"}]});
    assert!(hook(
        &h,
        "opencode",
        &opencode("db_query", json!({}), with_image),
        &[]
    )
    .1
    .is_none());
    assert!(hook(
        &h,
        "opencode",
        &opencode("edit", json!({}), json!({"output": orders()})),
        &[]
    )
    .1
    .is_none());
}

#[test]
fn every_agent_exits_with_zero_on_anything() {
    let h = home("zero");
    for agent in ["codex", "gemini", "copilot", "opencode"] {
        for input in ["", "{}", "[]", "null", "{\"tool_name\":", "\u{0}\u{1}"] {
            let (code, reply) = hook(&h, agent, input, &[]);
            assert_eq!((code, reply), (0, None), "{agent} {input:?}");
        }
    }
}

// ---- every agent gets what Claude Code gets -------------------------------------------------------

/// What the hook of Claude Code puts in place of `text`, for the command `command`.
fn claude_text(home: &Path, command: &str, text: &str) -> Option<String> {
    let ev = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash",
                    "tool_input": {"command": command},
                    "tool_response": {"stdout": text, "stderr": "", "interrupted": false, "isImage": false}})
    .to_string();
    let (_, reply) = hook(home, "claude", &ev, &[]);
    reply?["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .map(String::from)
}

/// The corpus is the test: for each fixture that Claude Code summarizes, each other agent is
/// handed the same output in its own event, and must answer with the same summary in its own reply.
/// A difference means that an adapter changes what the model sees, which it must not.
#[test]
fn every_fixture_gets_the_same_summary_in_every_agent() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let h = home("same");
    let (mut compared, mut differ) = (0, Vec::new());
    for e in manifest["positive"].as_array().unwrap() {
        let file = e["file"].as_str().unwrap();
        let text = String::from_utf8_lossy(&fs::read(dir.join(file)).unwrap()).to_string();
        let command = "mytool --run";
        let Some(want) = claude_text(&h, command, &text) else {
            continue;
        };
        let codex = hook(
            &h,
            "codex",
            &codex("Bash", json!({"command": command}), json!(text)),
            &[],
        )
        .1;
        let copilot = hook(
            &h,
            "copilot",
            &copilot(json!({"command": command}), "success", &text),
            &[],
        )
        .1;
        let opencode = hook(
            &h,
            "opencode",
            &opencode("bash", json!({"command": command}), json!({"output": text})),
            &[],
        )
        .1;
        let gemini_llm = format!(
            "Command: {command}\nDirectory: (root)\nOutput: {text}\nError: (none)\nExit Code: 0"
        );
        let gemini = hook(
            &h,
            "gemini",
            &gemini(
                "run_shell_command",
                json!({"command": command}),
                json!(gemini_llm),
            ),
            &[],
        )
        .1;
        let got = [
            (
                "codex",
                codex.and_then(|r| r["reason"].as_str().map(String::from)),
            ),
            (
                "copilot",
                copilot.and_then(|r| {
                    r["modifiedResult"]["textResultForLlm"]
                        .as_str()
                        .map(String::from)
                }),
            ),
            (
                "opencode",
                opencode.and_then(|r| r["output"].as_str().map(String::from)),
            ),
            (
                "gemini",
                gemini.and_then(|r| {
                    r["reason"].as_str().and_then(|s| {
                        s.strip_prefix(&format!("Command: {command}\nDirectory: (root)\nOutput: "))
                            .and_then(|s| s.strip_suffix("\nError: (none)\nExit Code: 0"))
                            .map(String::from)
                    })
                }),
            ),
        ];
        for (agent, text) in got {
            if text.as_deref() != Some(want.as_str()) {
                differ.push(format!("{file}: {agent}"));
            }
        }
        compared += 1;
    }
    assert!(
        compared >= 40,
        "only {compared} fixtures were summarized: the test proves too little"
    );
    assert!(
        differ.is_empty(),
        "the summary differs from Claude Code's for:\n{}",
        differ.join("\n")
    );
}
