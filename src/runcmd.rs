//! `dtk run` and `dtk filter`: dtk for an agent that has no hook that can replace a tool's output.
//!
//! The Claude Code hook sees the output after the command ran and replaces it. Many agents cannot
//! do that, but can do one of these: rewrite the command before it runs (Cursor, Gemini CLI,
//! Copilot CLI, Codex, OpenCode all can), follow a rule in a file such as `AGENTS.md`, or run a
//! shell command that has a pipe in it. `dtk run -c 'CMD'` runs the command and prints what the
//! hook would have put in place of its output; `CMD | dtk filter` does the same for text that
//! is already there. Both make the decision with `hook::handle`, the function the Claude Code
//! hook uses, so the rules are the same: what was asked for exactly (`head`, `sed -n`), the
//! exclusions, `DTK_DISABLED`, and the files that dtk saves.
//!
//! A command that fails, an output that is not text, and an output that dtk would leave alone are
//! printed exactly as they were, and the exit code of the command is the exit code of `dtk run`.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// The most output that is held to be summarized. More than this is passed through as it comes.
const MAX_CAPTURE: usize = 64 * 1024 * 1024;

/// What dtk would put in place of `stdout` for the output of `command`, or `None` when it leaves
/// it alone. `request` is the words of what the user asked, when the agent can say.
pub fn summary_for(command: &str, stdout: &str, request: Option<&str>) -> Option<String> {
    let mut input = json!({"command": command});
    if let Some(r) = request.filter(|r| !r.trim().is_empty()) {
        input["description"] = json!(r);
    }
    let event = json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": input,
        "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false, "isImage": false},
    })
    .to_string();
    if crate::hook::is_dtk_query_event(&event) {
        return None;
    }
    let answer: Value = serde_json::from_str(&crate::hook::handle(&event)?).ok()?;
    answer["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .map(String::from)
}

fn disabled() -> bool {
    std::env::var("DTK_DISABLED").is_ok_and(|v| v == "1")
}

fn request_from(flag: Option<String>) -> Option<String> {
    flag.or_else(|| std::env::var("DTK_REQUEST").ok())
}

/// Prints `text`, and a newline when it has none.
fn print_summary(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    if !text.ends_with('\n') {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

fn write_raw(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

const USAGE: &str = "usage: dtk run [--request TEXT] -c 'COMMAND'\n       dtk run [--request TEXT] [--] PROGRAM [ARGS...]";

/// `dtk run [--request TEXT] -c 'CMD'` or `dtk run [--request TEXT] [--] PROG ARGS...`
pub fn run(args: &[String]) -> i32 {
    let mut request: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--request" => {
                request = args.get(i + 1).cloned();
                i += 2;
            }
            _ => break,
        }
    }
    let rest = &args[i..];
    let (shown, mut cmd): (String, Command) = match rest {
        [c, text, ..] if c == "-c" => {
            let mut cmd = shell_command();
            cmd.arg(text);
            (text.clone(), cmd)
        }
        [dd, prog, more @ ..] if dd == "--" => {
            let mut cmd = Command::new(prog);
            cmd.args(more);
            (rest[1..].join(" "), cmd)
        }
        [prog, more @ ..] if !prog.starts_with('-') => {
            let mut cmd = Command::new(prog);
            cmd.args(more);
            (rest.join(" "), cmd)
        }
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    cmd.stdin(Stdio::inherit()).stderr(Stdio::inherit());
    if disabled() {
        return match cmd.stdout(Stdio::inherit()).status() {
            Ok(s) => exit_code(s),
            Err(e) => {
                eprintln!("dtk run: cannot start the command: {e}");
                127
            }
        };
    }
    cmd.stdout(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dtk run: cannot start the command: {e}");
            return 127;
        }
    };
    let mut out = Vec::new();
    let mut overflow = false;
    if let Some(mut so) = child.stdout.take() {
        let _ = (&mut so).take(MAX_CAPTURE as u64 + 1).read_to_end(&mut out);
        if out.len() > MAX_CAPTURE {
            // Too much to hold: what has come is printed, and the rest goes straight through.
            overflow = true;
            write_raw(&out);
            let mut sink = std::io::stdout().lock();
            let _ = std::io::copy(&mut so, &mut sink);
            let _ = sink.flush();
        }
    }
    let code = child.wait().map_or(1, exit_code);
    if overflow {
        return code;
    }
    // A failed command shows its output as it was: that is what the model needs to see.
    let summary = if code == 0 {
        std::str::from_utf8(&out)
            .ok()
            .and_then(|text| summary_for(&shown, text, request_from(request).as_deref()))
    } else {
        None
    };
    match summary {
        Some(s) => print_summary(&s),
        None => write_raw(&out),
    }
    code
}

#[cfg(unix)]
fn shell_command() -> Command {
    let mut c = Command::new("/bin/sh");
    c.arg("-c");
    c
}

#[cfg(not(unix))]
fn shell_command() -> Command {
    let mut c = Command::new("cmd");
    c.arg("/C");
    c
}

/// `dtk filter [--command CMD] [--request TEXT]`: standard input, summarized as the hook would.
pub fn filter(args: &[String]) -> i32 {
    let (mut command, mut request) = (String::from("(standard input)"), None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--command" => command = it.next().cloned().unwrap_or(command),
            "--request" => request = it.next().cloned(),
            other => {
                eprintln!("dtk filter: unexpected argument {other}\nusage: dtk filter [--command CMD] [--request TEXT]");
                return 2;
            }
        }
    }
    let mut buf = Vec::new();
    let mut stdin = std::io::stdin().lock();
    let _ = (&mut stdin)
        .take(MAX_CAPTURE as u64 + 1)
        .read_to_end(&mut buf);
    if buf.len() > MAX_CAPTURE {
        write_raw(&buf);
        let mut sink = std::io::stdout().lock();
        let _ = std::io::copy(&mut stdin, &mut sink);
        return 0;
    }
    if disabled() {
        write_raw(&buf);
        return 0;
    }
    let summary = std::str::from_utf8(&buf)
        .ok()
        .and_then(|text| summary_for(&command, text, request_from(request).as_deref()));
    match summary {
        Some(s) => print_summary(&s),
        None => write_raw(&buf),
    }
    0
}
