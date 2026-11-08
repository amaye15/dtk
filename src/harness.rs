//! dtk for agents other than Claude Code.
//!
//! Every agent that has a hook can tell dtk what a tool printed. They differ in what they call
//! things and in how they take an answer. This module turns the event of each agent into the event
//! of Claude Code, hands it to `hook::handle` (so the rules are the same everywhere: what was asked
//! for exactly, the exclusions, the summaries, the files that dtk saves, the log of decisions), and
//! turns the answer back into the reply that the agent understands.
//!
//! Only a hook that replaces the result after the command ran is used. A hook that rewrites the
//! command (to `dtk run -c '...'`) changes the text that the agent's allow and deny rules see, and a
//! deny rule such as `rm *` could stop matching it. dtk does not do that on its own.
//!
//! | agent | event | tool | how the answer replaces the result |
//! |---|---|---|---|
//! | Codex CLI | `PostToolUse` | `Bash`, `mcp__*` | `{"decision":"block","reason":TEXT}` |
//! | Gemini CLI | `AfterTool` | `run_shell_command`, `mcp_*` | `{"decision":"deny","reason":TEXT}` |
//! | Copilot CLI | `postToolUse` | `bash` | `{"modifiedResult":{...TEXT}}` |
//! | Mistral Vibe | `post_tool` | `bash` | `{"decision":"deny","reason":TEXT}` |
//! | OpenCode, Kilo Code | `tool.execute.after` (a plugin) | `bash`, MCP tools | the plugin sets the output |
//! | Pi, Hermes | an extension or a plugin, after the tool | `bash`, `terminal` | the shim sets the output |
//!
//! OpenCode, Kilo Code, Pi and Hermes have no hook that runs a program, so each has a small shim
//! that calls `dtk hook opencode` (the name `plugin` is the same) with `{"tool","args","output"}`.
//!
//! Only Claude Code and OpenCode were tried with the real program; the others follow the
//! documentation of each agent, and are tested with payloads written as that documentation says.

use serde_json::{json, Value};
use std::io::Read;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agent {
    Codex,
    Gemini,
    Copilot,
    Vibe,
    OpenCode,
}

impl Agent {
    pub fn parse(name: &str) -> Option<Agent> {
        match name {
            "codex" => Some(Agent::Codex),
            "gemini" => Some(Agent::Gemini),
            "copilot" => Some(Agent::Copilot),
            "vibe" => Some(Agent::Vibe),
            "opencode" | "plugin" => Some(Agent::OpenCode),
            _ => None,
        }
    }
}

/// What a tool printed, in the terms of Claude Code, and how to put a summary back.
struct Seen {
    /// `Bash`, or `mcp__server__tool`.
    tool: String,
    command: String,
    description: Option<String>,
    text: String,
    /// The shape of the answer, which depends on what came in.
    reply: Reply,
}

enum Reply {
    /// The whole text is replaced.
    Text,
    /// Gemini wraps the output of a shell command in lines of its own: the text before the output
    /// and after it are kept, and only the output is replaced.
    GeminiShell { before: String, after: String },
    /// Copilot's own names for the fields (`tool_result`, not `toolResult`).
    CopilotPascal,
    /// An MCP result in OpenCode: a list of content blocks.
    Blocks,
}

fn string_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let texts: Option<Vec<&str>> = parts
                .iter()
                .map(|p| p.get("text").and_then(Value::as_str).or_else(|| p.as_str()))
                .collect();
            Some(texts?.join("\n"))
        }
        _ => None,
    }
}

/// The text of a tool response that is a string, or an object that holds the text under a name
/// that agents use for it.
fn response_text(v: &Value) -> Option<String> {
    if let Some(s) = string_of(v) {
        return Some(s);
    }
    let obj = v.as_object()?;
    for key in [
        "output",
        "stdout",
        "aggregated_output",
        "formatted_output",
        "text",
        "content",
        "llmContent",
        "textResultForLlm",
        "text_result_for_llm",
    ] {
        if let Some(s) = obj.get(key).and_then(string_of) {
            return Some(s);
        }
    }
    None
}

/// Gemini CLI prints a shell result as `Command: ...`, `Directory: ...`, `Output: ...`,
/// `Error: ...`, `Exit Code: ...`. The output is what lies between `Output: ` and the last
/// `Error: `.
fn gemini_shell_parts(llm: &str) -> Option<(String, String, String)> {
    let start = llm.find("\nOutput: ")? + "\nOutput: ".len();
    let end = llm.rfind("\nError: ").filter(|&e| e >= start)?;
    Some((
        llm[..start].to_string(),
        llm[start..end].to_string(),
        llm[end..].to_string(),
    ))
}

fn args_of(v: &Value) -> Option<Value> {
    match v {
        Value::Object(_) => Some(v.clone()),
        Value::String(s) => serde_json::from_str(s).ok(),
        _ => None,
    }
}

fn seen_codex(ev: &Value) -> Option<Seen> {
    if ev.get("hook_event_name")?.as_str()? != "PostToolUse" {
        return None;
    }
    let tool = ev.get("tool_name")?.as_str()?;
    let input = ev.get("tool_input");
    let text = response_text(ev.get("tool_response")?)?;
    if tool == "Bash" || tool == "exec_command" {
        Some(Seen {
            tool: "Bash".into(),
            command: input?
                .get("command")
                .or_else(|| input?.get("cmd"))?
                .as_str()?
                .into(),
            description: input
                .and_then(|i| i.get("description"))
                .and_then(Value::as_str)
                .map(String::from),
            text,
            reply: Reply::Text,
        })
    } else if tool.starts_with("mcp__") {
        Some(Seen {
            tool: tool.into(),
            command: String::new(),
            description: None,
            text,
            reply: Reply::Text,
        })
    } else {
        None
    }
}

fn seen_gemini(ev: &Value) -> Option<Seen> {
    if ev.get("hook_event_name")?.as_str()? != "AfterTool" {
        return None;
    }
    let tool = ev.get("tool_name")?.as_str()?;
    let response = ev.get("tool_response")?;
    let llm = response.get("llmContent").and_then(string_of)?;
    if tool == "run_shell_command" {
        let input = ev.get("tool_input")?;
        let (text, reply) = match gemini_shell_parts(&llm) {
            Some((before, out, after)) => (out, Reply::GeminiShell { before, after }),
            None => (llm, Reply::Text),
        };
        Some(Seen {
            tool: "Bash".into(),
            command: input.get("command")?.as_str()?.into(),
            description: input
                .get("description")
                .and_then(Value::as_str)
                .map(String::from),
            text,
            reply,
        })
    } else {
        tool.strip_prefix("mcp_").map(|rest| Seen {
            tool: format!("mcp__{rest}"),
            command: String::new(),
            description: None,
            text: llm,
            reply: Reply::Text,
        })
    }
}

fn seen_copilot(ev: &Value) -> Option<Seen> {
    let (tool, args, result, pascal) = match ev.get("toolName") {
        Some(t) => (
            t.as_str()?,
            ev.get("toolArgs")?,
            ev.get("toolResult")?,
            false,
        ),
        None => (
            ev.get("tool_name")?.as_str()?,
            ev.get("tool_input")?,
            ev.get("tool_result")?,
            true,
        ),
    };
    if !tool.eq_ignore_ascii_case("bash") && !tool.eq_ignore_ascii_case("powershell") {
        return None;
    }
    // A result that is not a success (a failure, a denial) is left as it is.
    let kind = result
        .get("resultType")
        .or_else(|| result.get("result_type"))
        .and_then(Value::as_str)
        .unwrap_or("success");
    if kind != "success" {
        return None;
    }
    let text = response_text(result)?;
    let args = args_of(args)?;
    Some(Seen {
        tool: "Bash".into(),
        command: args.get("command")?.as_str()?.into(),
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(String::from),
        text,
        reply: if pascal {
            Reply::CopilotPascal
        } else {
            Reply::Text
        },
    })
}

/// Mistral Vibe `post_tool`: `tool_name`, `tool_input`, `tool_status`, `tool_output` (a structured
/// object) and `tool_output_text` (what the model will see). A shell command that fails still has
/// the status `success`, so the exit code in `tool_output` is read, and a failure is left alone.
/// When `tool_output` holds `stdout`, that is the text; the reply replaces the whole text.
fn seen_vibe(ev: &Value) -> Option<Seen> {
    if ev.get("tool_name")?.as_str()? != "bash" {
        return None;
    }
    if ev
        .get("tool_status")
        .and_then(Value::as_str)
        .unwrap_or("success")
        != "success"
    {
        return None;
    }
    let out = ev.get("tool_output").filter(|o| o.is_object());
    let failed = out.is_some_and(|o| {
        ["returncode", "return_code", "exit_code", "exitCode"]
            .iter()
            .any(|k| o.get(k).and_then(Value::as_i64).is_some_and(|c| c != 0))
            || o.get("stderr")
                .and_then(Value::as_str)
                .is_some_and(|e| !e.trim().is_empty())
    });
    if failed {
        return None;
    }
    let text = out
        .and_then(|o| o.get("stdout"))
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| {
            ev.get("tool_output_text")
                .and_then(Value::as_str)
                .map(String::from)
        })?;
    let input = ev.get("tool_input")?;
    Some(Seen {
        tool: "Bash".into(),
        command: input.get("command")?.as_str()?.into(),
        description: None,
        text,
        reply: Reply::Text,
    })
}

/// OpenCode has no hook that runs a program. A plugin (`integrations/opencode/dtk.ts`) calls
/// `dtk hook opencode` with `{"tool", "args", "output"}`, where `output` is what the tool returned:
/// for `bash` `{"output": TEXT, ...}`, for an MCP tool `{"content": [{"type": "text", ...}]}`.
fn seen_opencode(ev: &Value) -> Option<Seen> {
    let tool = ev.get("tool")?.as_str()?;
    let output = ev.get("output")?;
    let args = ev.get("args");
    if tool == "bash" || tool == "shell" {
        Some(Seen {
            tool: "Bash".into(),
            command: args?.get("command")?.as_str()?.into(),
            description: args
                .and_then(|a| a.get("description"))
                .and_then(Value::as_str)
                .map(String::from),
            text: output.get("output")?.as_str()?.into(),
            reply: Reply::Text,
        })
    } else if let Some(blocks) = output.get("content").filter(|c| c.is_array()) {
        // Every block must be text: an image or a resource is left alone.
        let text = string_of(blocks).filter(|_| {
            blocks
                .as_array()
                .is_some_and(|b| b.iter().all(|p| p["type"] == "text"))
        })?;
        Some(Seen {
            tool: format!("mcp__{tool}"),
            command: String::new(),
            description: None,
            text,
            reply: Reply::Blocks,
        })
    } else {
        None
    }
}

/// The event of Claude Code for what was seen.
fn claude_event(seen: &Seen) -> String {
    let mut input = json!({});
    if seen.tool == "Bash" {
        input["command"] = json!(seen.command);
    }
    if let Some(d) = &seen.description {
        input["description"] = json!(d);
    }
    let response = if seen.tool == "Bash" {
        json!({"stdout": seen.text, "stderr": "", "interrupted": false, "isImage": false})
    } else {
        json!([{"type": "text", "text": seen.text}])
    };
    json!({
        "hook_event_name": "PostToolUse",
        "tool_name": seen.tool,
        "tool_input": input,
        "tool_response": response,
    })
    .to_string()
}

/// The text that dtk puts in place of the output, from the answer of `hook::handle`.
fn replacement(answer: &str) -> Option<String> {
    let v: Value = serde_json::from_str(answer).ok()?;
    let updated = &v["hookSpecificOutput"]["updatedToolOutput"];
    updated
        .get("stdout")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| string_of(updated))
}

/// The reply of `agent` that puts `text` in place of what the tool printed.
fn reply(agent: Agent, seen: &Seen, text: &str) -> Value {
    match (agent, &seen.reply) {
        (Agent::Codex, _) => json!({"decision": "block", "reason": text}),
        (Agent::Gemini, Reply::GeminiShell { before, after }) => {
            json!({"decision": "deny", "reason": format!("{before}{text}{after}")})
        }
        (Agent::Gemini, _) | (Agent::Vibe, _) => json!({"decision": "deny", "reason": text}),
        (Agent::Copilot, Reply::CopilotPascal) => json!({"modifiedResult":
            {"result_type": "success", "text_result_for_llm": text}}),
        (Agent::Copilot, _) => json!({"modifiedResult":
            {"resultType": "success", "textResultForLlm": text}}),
        (Agent::OpenCode, Reply::Blocks) => json!({"content": [{"type": "text", "text": text}]}),
        (Agent::OpenCode, _) => json!({"output": text}),
    }
}

fn translate(agent: Agent, ev: &Value) -> Option<Seen> {
    match agent {
        Agent::Codex => seen_codex(ev),
        Agent::Gemini => seen_gemini(ev),
        Agent::Copilot => seen_copilot(ev),
        Agent::Vibe => seen_vibe(ev),
        Agent::OpenCode => seen_opencode(ev),
    }
}

/// What the event of `agent` says was printed (the command and the text), and the reply that would
/// put `summary` in its place: the two ends of the translation, with no decision in the middle.
/// For `dtk verify`, which must not write files.
pub fn roundtrip(agent: Agent, input: &str, summary: &str) -> Option<(String, String, Value)> {
    let ev: Value = serde_json::from_str(input).ok()?;
    let seen = translate(agent, &ev)?;
    let r = reply(agent, &seen, summary);
    Some((seen.command, seen.text, r))
}

/// The reply of `agent` to `input`, or `None` to leave the result as it is.
pub fn respond(agent: Agent, input: &str) -> Option<String> {
    if std::env::var("DTK_DISABLED").is_ok_and(|v| v == "1") {
        return None;
    }
    let ev: Value = serde_json::from_str(input).ok()?;
    let seen = translate(agent, &ev)?;
    let event = claude_event(&seen);
    // The model asked for exactly these rows with a dtk command: they are not changed.
    if crate::hook::is_dtk_query_event(&event) {
        return None;
    }
    let text = replacement(&crate::hook::handle(&event)?)?;
    Some(reply(agent, &seen, &text).to_string())
}

/// `dtk hook AGENT`: reads the event on standard input, prints the reply (or nothing).
pub fn run(agent: Agent) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    let mut buf = Vec::new();
    let _ = std::io::stdin().take(8 * 1024 * 1024).read_to_end(&mut buf);
    let input = String::from_utf8_lossy(&buf).to_string();
    if let Some(out) = std::panic::catch_unwind(|| respond(agent, &input))
        .ok()
        .flatten()
    {
        println!("{out}");
    }
    0
}
