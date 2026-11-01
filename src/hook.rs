//! The Claude Code `PostToolUse` hook plus the small commands around it (`view`, `gain`, `mode`).
//!
//! The hook fails open: any problem means "print nothing, exit 0", so Claude Code keeps the
//! original output.
//!
//! With `--then CMD ARGS...` the hook is a front door: dtk handles tables, and hands every other
//! event, byte for byte, to CMD (normally `rtk hook claude`). One registered hook, a fixed order.

use dtk::lean::{self, est_tokens, Ctx, Rendered};
use dtk::table::{self, Format, Table};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Smallest output (estimated tokens) worth replacing when Claude Code shows it in full.
const MIN_TOKENS: usize = 1200;
/// What the model sees for an output Claude Code saved to a file: a short preview and a path.
const STUB_TOKENS: usize = 580;
/// Largest summary, in estimated tokens.
const BUDGET: usize = 2500;
/// Fewest rows for which a saved-to-file output is replaced.
const MIN_PERSISTED_ROWS: usize = 30;
const KEEP_FILES: usize = 60;
const KEEP_SECS: u64 = 14 * 24 * 3600;
/// Longest the chained command may run.
const CHAIN_TIMEOUT_MS: u64 = 10_000;

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn data_dir() -> Option<PathBuf> {
    Some(home()?.join(".local/share/dtk"))
}

pub fn config_dir() -> Option<PathBuf> {
    Some(home()?.join(".config/dtk"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(unix)]
pub fn create_private_dir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(p)
}

#[cfg(not(unix))]
pub fn create_private_dir(p: &Path) -> std::io::Result<()> {
    fs::create_dir_all(p)
}

#[cfg(unix)]
fn open_private(p: &Path, append: bool) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = fs::OpenOptions::new();
    o.create(true).mode(0o600);
    if append {
        o.append(true);
    } else {
        o.write(true).truncate(true);
    }
    o.open(p)
}

#[cfg(not(unix))]
fn open_private(p: &Path, append: bool) -> std::io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.create(true);
    if append {
        o.append(true);
    } else {
        o.write(true).truncate(true);
    }
    o.open(p)
}

/// Writes `bytes` to the store under a content hash, so the same output maps to the same file.
fn store_write(ext: &str, bytes: &[u8]) -> Option<PathBuf> {
    let dir = data_dir()?.join("store");
    create_private_dir(&dir).ok()?;
    let name = format!("{}.{}", &hex(&Sha256::digest(bytes))[..12], ext);
    let path = dir.join(name);
    if !path.exists() {
        open_private(&path, false).ok()?.write_all(bytes).ok()?;
    }
    prune(&dir);
    Some(path)
}

/// Keeps the store small: newest files only, nothing older than two weeks.
fn prune(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let now = SystemTime::now();
    for (i, (m, p)) in files.iter().enumerate() {
        let old = now
            .duration_since(*m)
            .map(|d| d.as_secs() > KEEP_SECS)
            .unwrap_or(false);
        if i >= KEEP_FILES || old {
            let _ = fs::remove_file(p);
        }
    }
}

fn log_decision(entry: Value) {
    let Some(dir) = data_dir() else { return };
    if create_private_dir(&dir).is_err() {
        return;
    }
    let path = dir.join("decisions.jsonl");
    trim_log(&path);
    if let Ok(mut f) = open_private(&path, true) {
        let _ = writeln!(f, "{}", entry);
    }
}

/// Keeps the decision log under about 4 MB by dropping the oldest lines.
fn trim_log(path: &Path) {
    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    const KEEP_LINES: usize = 5000;
    if fs::metadata(path).map(|m| m.len()).unwrap_or(0) <= MAX_BYTES {
        return;
    }
    if let Ok(text) = fs::read_to_string(path) {
        let lines: Vec<&str> = text.lines().collect();
        let keep = lines[lines.len().saturating_sub(KEEP_LINES)..].join("\n");
        if let Ok(mut f) = open_private(path, false) {
            let _ = writeln!(f, "{}", keep);
        }
    }
}

/// `live` unless `DTK_MODE=shadow` or the mode file says `shadow`.
pub fn is_live() -> bool {
    if let Ok(m) = std::env::var("DTK_MODE") {
        return m != "shadow";
    }
    match config_dir().and_then(|d| fs::read_to_string(d.join("mode")).ok()) {
        Some(s) => s.trim() != "shadow",
        None => true,
    }
}

/// Only read files that Claude Code itself saved.
fn under_claude_dir(p: &Path) -> bool {
    let Some(h) = home() else { return false };
    match (p.canonicalize(), h.join(".claude").canonicalize()) {
        (Ok(p), Ok(base)) => p.starts_with(base),
        _ => false,
    }
}

/// Renders the lean view and, when `write` is set, saves the full data next to it.
fn build(t: &Table, text: &str, persisted: Option<&Path>, write: bool) -> Rendered {
    let mut ctx = Ctx {
        bytes: text.len(),
        csv_path: None,
        raw_path: None,
    };
    if t.rows.len() > lean::LOSSLESS_ROWS {
        let is_json = matches!(t.format, Format::Jsonl | Format::JsonArray);
        if t.format == Format::Csv && persisted.is_some() {
            ctx.csv_path = persisted.map(|p| p.display().to_string());
        } else {
            let saved = if write {
                store_write("csv", lean::to_csv(t).as_bytes())
            } else {
                None
            };
            ctx.csv_path = Some(
                saved
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<store>/data.csv".to_string()),
            );
        }
        if is_json {
            ctx.raw_path = match persisted {
                Some(p) => Some(p.display().to_string()),
                None => {
                    let ext = if t.format == Format::Jsonl {
                        "jsonl"
                    } else {
                        "json"
                    };
                    let saved = if write {
                        store_write(ext, text.as_bytes())
                    } else {
                        None
                    };
                    Some(
                        saved
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| format!("<store>/raw.{ext}")),
                    )
                }
            };
        }
    }
    lean::render(t, &ctx, BUDGET)
}

/// `dtk hook claude [--then CMD ARGS...]`
pub fn run(args: &[String]) -> i32 {
    if args.first().map(String::as_str) != Some("claude") {
        eprintln!("usage: dtk hook claude [--then CMD ARGS...]");
        return 0;
    }
    let then: Vec<String> = match args.iter().position(|a| a == "--then") {
        Some(i) => args[i + 1..].to_vec(),
        None => Vec::new(),
    };
    std::panic::set_hook(Box::new(|_| {}));
    let mut buf = Vec::new();
    let _ = std::io::stdin().take(8 * 1024 * 1024).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let handled = std::panic::catch_unwind(|| handle(&text)).ok().flatten();
    if let Some(out) = handled {
        println!("{out}");
        return 0;
    }
    if then.is_empty() {
        return 0;
    }
    run_chain(&then, &buf)
}

/// Runs the chained hook with the original event and copies its answer.
fn run_chain(argv: &[String], input: &[u8]) -> i32 {
    let Some((prog, rest)) = argv.split_first() else {
        return 0;
    };
    let Ok(mut child) = Command::new(prog)
        .args(rest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    else {
        return 0;
    };
    // The reader fills a shared buffer and is never joined: if the chained command leaves a
    // child process holding the pipe open, dtk must still return on time.
    let collected = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    if let Some(mut o) = child.stdout.take() {
        let sink = Arc::clone(&collected);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match o.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut b) = sink.lock() {
                            b.extend_from_slice(&chunk[..n]);
                        }
                    }
                }
            }
            let _ = done_tx.send(());
        });
    }
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(input);
    }
    let limit = std::env::var("DTK_CHAIN_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(CHAIN_TIMEOUT_MS);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() > Duration::from_millis(limit) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    let Some(status) = status else { return 0 };
    // The command has exited; give the reader a moment to drain the pipe, then take what we have.
    let _ = done_rx.recv_timeout(Duration::from_millis(500));
    let bytes = collected.lock().map(|b| b.clone()).unwrap_or_default();
    let _ = std::io::stdout().write_all(&bytes);
    status.code().unwrap_or(0)
}

/// The decision for one event. Returns the JSON to print, or `None` to leave the output alone.
fn handle(input: &str) -> Option<String> {
    if std::env::var("DTK_DISABLED")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return None;
    }
    let ev: Value = serde_json::from_str(input).ok()?;
    if ev.get("hook_event_name")?.as_str()? != "PostToolUse"
        || ev.get("tool_name")?.as_str()? != "Bash"
    {
        return None;
    }
    let resp = ev.get("tool_response")?.as_object()?;
    if resp.get("isImage").and_then(Value::as_bool) == Some(true)
        || resp.get("interrupted").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let stdout = resp.get("stdout")?.as_str()?;
    let persisted: Option<PathBuf> = resp
        .get("persistedOutputPath")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let text: String = match &persisted {
        Some(p) if under_claude_dir(p) => fs::read_to_string(p).ok()?,
        Some(_) => return None,
        None => stdout.to_string(),
    };
    let table = table::detect(&text)?;
    let live = is_live();
    let tokens_full = est_tokens(&text);
    let baseline = if persisted.is_some() {
        STUB_TOKENS
    } else {
        tokens_full
    };
    let rendered = build(&table, &text, persisted.as_deref(), live);
    let after = est_tokens(&rendered.text);
    let would_apply = if persisted.is_some() {
        table.rows.len() >= MIN_PERSISTED_ROWS
    } else {
        tokens_full >= MIN_TOKENS && after * 10 <= tokens_full * 7
    };
    let applied = live && would_apply;
    let cmd: String = ev
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(100)
        .collect();
    log_decision(json!({
        "ts": now_secs(),
        "mode": if live { "live" } else { "shadow" },
        "format": table.format.name(),
        "rows": table.rows.len(),
        "cols": table.headers.len(),
        "persisted": persisted.is_some(),
        "lossless": rendered.lossless,
        "tokens_full": tokens_full,
        "tokens_baseline": baseline,
        "tokens_after": after,
        "would_apply": would_apply,
        "applied": applied,
        "cmd": cmd,
    }));
    if !applied {
        return None;
    }
    let mut updated = resp.clone();
    updated.insert("stdout".to_string(), Value::String(rendered.text));
    updated.remove("persistedOutputPath");
    updated.remove("persistedOutputSize");
    let out = json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "updatedToolOutput": Value::Object(updated)}});
    Some(out.to_string())
}

/// `dtk view [FILE]`: shows the lean view of a file or of standard input.
pub fn view(arg: Option<&str>) -> i32 {
    let (text, path) = match arg {
        Some(p) if p != "-" => match fs::read_to_string(p) {
            Ok(s) => (s, Path::new(p).canonicalize().ok()),
            Err(e) => {
                eprintln!("dtk: cannot read {p}: {e}");
                return 1;
            }
        },
        _ => {
            let mut s = String::new();
            if std::io::stdin().read_to_string(&mut s).is_err() {
                return 1;
            }
            (s, None)
        }
    };
    let Some(t) = table::detect(&text) else {
        eprintln!(
            "dtk: no table, CSV, JSON array or JSONL found (needs at least {} rows)",
            table::MIN_ROWS
        );
        return 1;
    };
    let r = build(&t, &text, path.as_deref(), true);
    println!("{}", r.text);
    eprintln!(
        "dtk: {} est. tokens -> {}",
        est_tokens(&text),
        est_tokens(&r.text)
    );
    0
}

/// `dtk mode [shadow|live]`: shows or sets the mode.
pub fn mode(arg: Option<&str>) -> i32 {
    let Some(dir) = config_dir() else { return 1 };
    match arg {
        None => {
            println!("{}", if is_live() { "live" } else { "shadow" });
            0
        }
        Some(m @ ("shadow" | "live")) => {
            if create_private_dir(&dir).is_err()
                || fs::write(dir.join("mode"), format!("{m}\n")).is_err()
            {
                eprintln!("dtk: cannot write the mode file");
                return 1;
            }
            println!("dtk mode: {m}");
            0
        }
        Some(_) => {
            eprintln!("usage: dtk mode [shadow|live]");
            2
        }
    }
}

/// rtk's own totals, when rtk is installed: (tokens saved, commands).
fn rtk_totals() -> Option<(i64, i64)> {
    let out = Command::new("rtk")
        .args(["gain", "-f", "json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let s = v.get("summary")?;
    Some((
        s.get("total_saved")?.as_i64()?,
        s.get("total_commands")?.as_i64()?,
    ))
}

/// `dtk gain`: totals from the decision log, with rtk's totals beside them.
pub fn gain() -> i32 {
    let Some(dir) = data_dir() else { return 1 };
    let log = fs::read_to_string(dir.join("decisions.jsonl")).unwrap_or_default();
    let (mut applied_mid, mut saved_mid, mut applied_big, mut after_big) = (0u64, 0i64, 0u64, 0u64);
    let (mut would, mut would_saved, mut seen) = (0u64, 0i64, 0u64);
    let mut by_format: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        seen += 1;
        let g = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
        let baseline = g("tokens_baseline");
        let after = g("tokens_after");
        let persisted = v.get("persisted").and_then(Value::as_bool).unwrap_or(false);
        *by_format
            .entry(
                v.get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            )
            .or_default() += 1;
        if v.get("applied").and_then(Value::as_bool) == Some(true) {
            if persisted {
                applied_big += 1;
                after_big += after as u64;
            } else {
                applied_mid += 1;
                saved_mid += baseline - after;
            }
        } else if v.get("would_apply").and_then(Value::as_bool) == Some(true) {
            would += 1;
            would_saved += baseline - after;
        }
    }
    println!(
        "dtk gain: {seen} tables seen, mode {}",
        if is_live() { "live" } else { "shadow" }
    );
    println!("replaced, shown in full by Claude Code: {applied_mid} outputs, {saved_mid} est. tokens saved");
    println!(
        "replaced, saved to a file by Claude Code: {applied_big} outputs; summaries total {after_big} est. tokens (each replaces a ~{STUB_TOKENS}-token preview and covers all rows)"
    );
    if would > 0 {
        println!("shadow only (would replace): {would} outputs, {would_saved} est. tokens");
    }
    if !by_format.is_empty() {
        println!(
            "by format: {}",
            by_format
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    match rtk_totals() {
        Some((saved, cmds)) => println!(
            "rtk (rtk gain): {saved} tokens saved over {cmds} commands. dtk adds on top: {saved_mid} est. tokens from tables."
        ),
        None => println!("rtk: not found, or `rtk gain` failed"),
    }
    0
}
