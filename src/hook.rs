//! The Claude Code `PostToolUse` hook plus the small commands around it (`view`, `gain`, `mode`).
//!
//! The hook fails open: any problem means "print nothing, exit 0", so Claude Code keeps the
//! original output.
//!
//! With `--then CMD ARGS...` the hook is a front door: dtk handles tables, and hands every other
//! event, byte for byte, to CMD (normally `rtk hook claude`). One registered hook, a fixed order.

use crate::config::Config;
use dtk::lean::{self, est_tokens, Ctx, Rendered};
use dtk::outline;
use dtk::table::{self, Format, Table};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the model sees for an output Claude Code saved to a file: a short preview and a path.
const STUB_TOKENS: usize = 580;
const KEEP_FILES: usize = 60;
const KEEP_SECS: u64 = 14 * 24 * 3600;
/// Longest the chained command may run.
const CHAIN_TIMEOUT_MS: u64 = 10_000;

/// The home folder: `HOME`, or `USERPROFILE` on Windows.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
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
    // symlink_metadata: a link whose target was deleted still has an age and can be pruned.
    let mut files: Vec<(SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.path().symlink_metadata().ok()?.modified().ok()?, e.path())))
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

/// Is `dtk` on the PATH, and is it this program? Summaries tell the model to run `dtk ...`.
pub fn dtk_on_path() -> bool {
    let Ok(me) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return false;
    };
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        ["dtk", "dtk.exe"]
            .iter()
            .any(|n| dir.join(n).canonicalize().is_ok_and(|c| c == me))
    })
}

/// Only read files that Claude Code itself saved.
fn under_claude_dir(p: &Path) -> bool {
    let Some(h) = home() else { return false };
    match (p.canonicalize(), h.join(".claude").canonicalize()) {
        (Ok(p), Ok(base)) => p.starts_with(base),
        _ => false,
    }
}

/// A long path (Claude Code keeps big outputs deep under `~/.claude/projects/...`) would be
/// repeated in several lines of a summary. Give it a short name in the store: a symlink.
fn short_path(target: &Path, ext: &str, write: bool) -> String {
    let long = target.display().to_string();
    if long.len() <= 60 || !write {
        return long;
    }
    #[cfg(unix)]
    {
        let Some(dir) = data_dir().map(|d| d.join("store")) else {
            return long;
        };
        if create_private_dir(&dir).is_err() {
            return long;
        }
        let link = dir.join(format!(
            "{}.{}",
            &hex(&Sha256::digest(long.as_bytes()))[..12],
            ext
        ));
        if fs::read_link(&link).ok().as_deref() != Some(target) {
            let _ = fs::remove_file(&link);
            if std::os::unix::fs::symlink(target, &link).is_err() {
                return long;
            }
        }
        link.display().to_string()
    }
    #[cfg(not(unix))]
    {
        let _ = ext; // no symlinks here: the long path is used
        long
    }
}

/// Renders the lean view and, when `write` is set, saves the full data next to it.
pub fn build(
    t: &Table,
    text: &str,
    persisted: Option<&Path>,
    write: bool,
    budget: usize,
) -> Rendered {
    let mut ctx = Ctx {
        bytes: text.len(),
        csv_path: None,
        raw_path: None,
    };
    if t.rows.len() > lean::LOSSLESS_ROWS {
        let is_json = matches!(t.format, Format::Jsonl | Format::JsonArray);
        if t.format == Format::Csv && persisted.is_some() {
            ctx.csv_path = persisted.map(|p| short_path(p, "csv", write));
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
                Some(p) => Some(short_path(
                    p,
                    if t.format == Format::Jsonl {
                        "jsonl"
                    } else {
                        "json"
                    },
                    write,
                )),
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
    lean::render(t, &ctx, budget)
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
    // The model asked for exactly these rows with a dtk command. Neither dtk nor rtk's pipeline
    // may change them, so the event is not passed on.
    if std::panic::catch_unwind(|| is_dtk_query_event(&text)).unwrap_or(false) {
        return 0;
    }
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
    // Check often at first (hooks usually answer in a few milliseconds), then back off.
    let mut nap = Duration::from_micros(200);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() > Duration::from_millis(limit) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => {
                std::thread::sleep(nap);
                nap = (nap * 2).min(Duration::from_millis(10));
            }
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
    if ev.get("hook_event_name")?.as_str()? != "PostToolUse" {
        return None;
    }
    match ev.get("tool_name")?.as_str()? {
        "Bash" => handle_bash(&ev),
        "Read" => handle_read(&ev),
        _ => None,
    }
}

/// Commands that ask for specific rows must get them, not a summary of them.
/// Is this PostToolUse event the result of one of dtk's own commands?
fn is_dtk_query_event(event: &str) -> bool {
    let Ok(ev) = serde_json::from_str::<Value>(event) else {
        return false;
    };
    ev.get("tool_name").and_then(Value::as_str) == Some("Bash")
        && ev
            .get("tool_input")
            .and_then(|i| i.get("command"))
            .and_then(Value::as_str)
            .is_some_and(|c| runs_dtk_query(&crate::rtk::plain_command(c)))
}

/// `dtk top FILE amount`, `/path/to/dtk where ...`: the model reading more of the data.
fn runs_dtk_query(cmd: &str) -> bool {
    let words: Vec<&str> = cmd
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | ';' | '&' | '(' | '\'' | '"'))
        .filter(|w| !w.is_empty())
        .collect();
    words.windows(2).any(|w| {
        let base = w[0].rsplit(['/', '\\']).next().unwrap_or("");
        (base == "dtk" || base == "dtk.exe") && crate::query::COMMANDS.contains(&w[1])
    })
}

pub fn wants_raw(cmd: &str) -> bool {
    // `--head-lines` and `--tail-lines` are how rtk rewrites `head` and `tail`.
    const MARKERS: [&str; 7] = [
        "DTK_RAW",
        "qsv slice",
        "qsv sample",
        "sed -n",
        "--max-lines",
        "--head-lines",
        "--tail-lines",
    ];
    if MARKERS.iter().any(|m| cmd.contains(m)) {
        return true;
    }
    cmd.split(|c: char| c.is_whitespace() || matches!(c, '|' | ';' | '&'))
        .any(|t| t == "head" || t == "tail")
}

/// What came in: the text of a tool result, and where it came from.
pub struct Input<'a> {
    /// `Bash` or `Read`.
    pub tool: &'a str,
    /// The command, or the file path for `Read`.
    pub subject: &'a str,
    pub text: &'a str,
    /// A file on disk that holds this very data, so the summary can point at it.
    pub file: Option<&'a Path>,
    /// Claude Code saved the output to a file and shows the model only a short preview.
    pub persisted: bool,
}

/// What dtk makes of an [`Input`]. The hook, `explain`, `verify` and `discover` all use this one
/// function, so what they say about a decision is what the hook does.
/// What dtk found in an output: a table, or a JSON document that gets an outline.
pub struct Detected {
    pub name: &'static str,
    pub rows: usize,
    pub cols: usize,
}

pub struct Verdict {
    pub table: Option<Table>,
    pub detected: Option<Detected>,
    pub rendered: Option<Rendered>,
    pub tokens_full: usize,
    /// What the model would see without dtk.
    pub baseline: usize,
    /// What the model sees with the summary.
    pub after: usize,
    pub applies: bool,
    pub reason: String,
}

pub fn evaluate(inp: &Input, cfg: &Config, write: bool) -> Verdict {
    let tokens_full = est_tokens(inp.text);
    let mut v = Verdict {
        table: None,
        detected: None,
        rendered: None,
        tokens_full,
        baseline: if inp.persisted {
            STUB_TOKENS
        } else {
            tokens_full
        },
        after: 0,
        applies: false,
        reason: String::new(),
    };
    if std::env::var("DTK_DISABLED")
        .map(|x| x == "1")
        .unwrap_or(false)
    {
        v.reason = "DTK_DISABLED=1 is set".into();
        return v;
    }
    // rtk's PreToolUse hook may have rewritten the command (`head -n 5 f` is now
    // `rtk read f --head-lines 5`). The user's intent and config refer to what they typed.
    let typed = crate::rtk::plain_command(inp.subject);
    if inp.tool == "Bash" && (wants_raw(inp.subject) || wants_raw(&typed)) {
        v.reason =
            "the command asks for exact rows (DTK_RAW, head, tail, sed -n, qsv slice, ...)".into();
        return v;
    }
    if cfg.is_excluded(inp.subject) || cfg.is_excluded(&typed) {
        v.reason = "excluded by exclude_commands in config.toml".into();
        return v;
    }
    // The model is drilling into a file that dtk saved: it asked for exactly these rows or values.
    if inp.tool == "Bash"
        && (inp.subject.replace('\\', "/").contains("/dtk/store/") || runs_dtk_query(&typed))
    {
        v.reason = "the command reads a file that dtk saved (a drill-down): shown as is".into();
        return v;
    }
    if let Some(marker) = crate::rtk::truncation_marker(inp.text) {
        let marker: String = marker.chars().take(60).collect();
        v.reason = format!(
            "rtk already cut this output (\"{marker}\"); a summary would claim to cover rows that are missing, and would hide rtk's pointer to the rest"
        );
        return v;
    }
    let Some(table) = table::detect(inp.text) else {
        if let Some(doc) = outline::parse_document(inp.text) {
            return evaluate_outline(inp, cfg, write, v, &doc);
        }
        v.reason = format!(
            "not a table: no JSONL, JSON, CSV, logfmt, access log, or aligned or bordered table found (needs at least {} rows)",
            table::MIN_ROWS
        );
        return v;
    };
    let rendered = build(&table, inp.text, inp.file, write, cfg.budget);
    v.after = est_tokens(&rendered.text);
    let rows = table.rows.len();
    let (applies, reason) = if inp.persisted {
        if rows >= cfg.min_persisted_rows {
            (true, format!("saved-to-file output: the summary covers all {rows} rows and replaces a preview of about {STUB_TOKENS} tokens"))
        } else {
            (
                false,
                format!(
                    "only {rows} rows (saved-to-file output needs {})",
                    cfg.min_persisted_rows
                ),
            )
        }
    } else if v.tokens_full < cfg.min_tokens {
        (
            false,
            format!(
                "small output ({} est. tokens, threshold {}): shown as is",
                v.tokens_full, cfg.min_tokens
            ),
        )
    } else if v.after * 10 > v.tokens_full * 7 {
        (
            false,
            format!(
                "the summary ({} tokens) would not be at least 30% smaller than the output ({})",
                v.after, v.tokens_full
            ),
        )
    } else {
        (
            true,
            format!(
                "the summary is {}% smaller",
                100 - v.after * 100 / v.tokens_full.max(1)
            ),
        )
    };
    v.applies = applies;
    v.reason = reason;
    v.detected = Some(Detected {
        name: table.format.name(),
        rows: table.rows.len(),
        cols: table.headers.len(),
    });
    v.table = Some(table);
    v.rendered = Some(rendered);
    v
}

/// A JSON document that is not a table gets an outline, but only when that spares the model a
/// lot: a document Claude Code saved to a file (the model would see a preview of its first
/// lines), or a big one. A small document is read in full, because its values are the point.
fn evaluate_outline(
    inp: &Input,
    cfg: &Config,
    write: bool,
    mut v: Verdict,
    doc: &serde_json::Value,
) -> Verdict {
    // A jq query asks for particular values; an outline would hide them.
    let typed = crate::rtk::plain_command(inp.subject);
    if inp.tool == "Bash"
        && typed
            .split(|c: char| c.is_whitespace() || matches!(c, '|' | ';' | '&' | '(' | '<'))
            .any(|w| matches!(w, "jq" | "jaq" | "gron" | "yq"))
    {
        v.reason =
            "a jq-style query: the model asked for these values, so they are shown as is".into();
        return v;
    }
    // Reading a file on purpose is a stronger request than a command that happens to print JSON.
    let floor = if inp.tool == "Read" {
        cfg.min_outline_tokens * 2
    } else {
        cfg.min_outline_tokens
    };
    let big_enough = inp.persisted || v.tokens_full >= floor;
    // Where the full document is, for the drill commands.
    let file = match inp.file {
        Some(p) => Some(short_path(p, "json", write)),
        None if write => store_write("json", inp.text.as_bytes()).map(|p| p.display().to_string()),
        None => None,
    };
    let text = outline::outline(doc, inp.text.len(), file.as_deref(), cfg.budget);
    v.after = est_tokens(&text);
    v.detected = Some(Detected {
        name: "json-document",
        rows: match doc {
            serde_json::Value::Array(a) => a.len(),
            serde_json::Value::Object(m) => m.len(),
            _ => 0,
        },
        cols: 0,
    });
    v.rendered = Some(Rendered {
        text,
        lossless: false,
    });
    (v.applies, v.reason) = if !big_enough {
        (
            false,
            format!(
                "a JSON document of {} est. tokens (outlines start at {}): shown as is, because its values are the point",
                v.tokens_full, floor
            ),
        )
    } else if !inp.persisted && v.after * 2 > v.tokens_full {
        (
            false,
            format!(
                "the outline ({} tokens) would not be at least half the size of the document ({})",
                v.after, v.tokens_full
            ),
        )
    } else if inp.persisted {
        (
            true,
            format!(
                "saved-to-file JSON document: the outline replaces a preview of about {STUB_TOKENS} tokens"
            ),
        )
    } else {
        (
            true,
            format!(
                "the outline is {}% smaller",
                100 - v.after * 100 / v.tokens_full.max(1)
            ),
        )
    };
    v
}

/// One line of the decision log.
struct Decision<'a> {
    tool: &'a str,
    what: String,
    verdict: &'a Verdict,
    persisted: bool,
    applied: bool,
    live: bool,
}

fn log_table(d: &Decision) {
    let (Some(found), Some(rendered)) = (&d.verdict.detected, &d.verdict.rendered) else {
        return;
    };
    log_decision(json!({
        "ts": now_secs(),
        "mode": if d.live { "live" } else { "shadow" },
        "tool": d.tool,
        "format": found.name,
        "rows": found.rows,
        "cols": found.cols,
        "persisted": d.persisted,
        "lossless": rendered.lossless,
        "tokens_full": d.verdict.tokens_full,
        "tokens_baseline": d.verdict.baseline,
        "tokens_after": d.verdict.after,
        "would_apply": d.verdict.applies,
        "applied": d.applied,
        "cmd": d.what,
    }));
}

/// File types that can hold a table. Source code is never read or checked.
pub const DATA_EXTS: [&str; 11] = [
    "csv", "tsv", "psv", "jsonl", "ndjson", "json", "log", "txt", "dat", "out", "tbl",
];

/// The `Read` tool on a data file. By now `Read` has passed the permission rules, so replacing
/// its content cannot reveal anything the model was not allowed to read.
fn handle_read(ev: &Value) -> Option<String> {
    let resp = ev.get("tool_response")?.as_object()?;
    let file = resp.get("file")?.as_object()?;
    let path = file.get("filePath")?.as_str()?;
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    if !DATA_EXTS.contains(&ext.as_str()) {
        return None;
    }
    // A range is a request for exact lines: leave it alone.
    let input = ev.get("tool_input");
    if input.and_then(|i| i.get("offset")).is_some() || input.and_then(|i| i.get("limit")).is_some()
    {
        return None;
    }
    let lines = file.get("numLines")?.as_u64()?;
    if file.get("startLine").and_then(Value::as_u64).unwrap_or(1) != 1
        || file.get("totalLines")?.as_u64()? != lines
    {
        return None;
    }
    let text = file.get("content")?.as_str()?;
    let live = is_live();
    let cfg = Config::load().0;
    let v = evaluate(
        &Input {
            tool: "Read",
            subject: path,
            text,
            file: Some(Path::new(path)),
            persisted: false,
        },
        &cfg,
        live,
    );
    let applied = live && v.applies;
    log_table(&Decision {
        tool: "Read",
        what: path.chars().take(100).collect(),
        verdict: &v,
        persisted: false,
        applied,
        live,
    });
    if !applied {
        return None;
    }
    let shown_text = v.rendered?.text;
    let shown = shown_text.lines().count();
    let mut new_file = file.clone();
    new_file.insert("content".to_string(), Value::String(shown_text));
    new_file.insert("numLines".to_string(), json!(shown));
    new_file.insert("totalLines".to_string(), json!(shown));
    let mut updated = resp.clone();
    updated.insert("file".to_string(), Value::Object(new_file));
    let out = json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "updatedToolOutput": Value::Object(updated)}});
    Some(out.to_string())
}

fn handle_bash(ev: &Value) -> Option<String> {
    let command = ev
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("");
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
    let live = is_live();
    let cfg = Config::load().0;
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: command,
            text: &text,
            file: persisted.as_deref(),
            persisted: persisted.is_some(),
        },
        &cfg,
        live,
    );
    let applied = live && v.applies;
    log_table(&Decision {
        tool: "Bash",
        what: command.chars().take(100).collect(),
        verdict: &v,
        persisted: persisted.is_some(),
        applied,
        live,
    });
    if !applied {
        return None;
    }
    let mut updated = resp.clone();
    updated.insert("stdout".to_string(), Value::String(v.rendered?.text));
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
        if let Some(doc) = outline::parse_document(&text) {
            let file = match &path {
                Some(p) => Some(short_path(p, "json", true)),
                None => store_write("json", text.as_bytes()).map(|p| p.display().to_string()),
            };
            let out = outline::outline(&doc, text.len(), file.as_deref(), Config::load().0.budget);
            println!("{out}");
            eprintln!(
                "dtk: {} est. tokens -> {}",
                est_tokens(&text),
                est_tokens(&out)
            );
            return 0;
        }
        eprintln!(
            "dtk: no table, CSV, JSON array, JSONL or JSON document found (a table needs at least {} rows)",
            table::MIN_ROWS
        );
        return 1;
    };
    let r = build(&t, &text, path.as_deref(), true, Config::load().0.budget);
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
pub fn gain(as_json: bool) -> i32 {
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
    if as_json {
        let rtk =
            rtk_totals().map(|(saved, cmds)| json!({"tokens_saved": saved, "commands": cmds}));
        println!(
            "{}",
            json!({
                "tables_seen": seen,
                "mode": if is_live() { "live" } else { "shadow" },
                "shown_in_full": {"outputs": applied_mid, "est_tokens_saved": saved_mid},
                "saved_to_file": {"outputs": applied_big, "summary_tokens": after_big, "replaces_preview_tokens": STUB_TOKENS},
                "shadow_only": {"outputs": would, "est_tokens_saved": would_saved},
                "by_format": by_format,
                "rtk": rtk,
            })
        );
        return 0;
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
