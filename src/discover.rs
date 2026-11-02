//! `dtk discover`: scans your Claude Code transcripts for tables that dtk would have summarized,
//! and says how many tokens that would have saved. It changes nothing.
//!
//! Like `rtk discover`, it works from history, so you can see what dtk is worth before (or
//! after) installing it. It uses `hook::evaluate`, the same decision the hook makes.

use crate::config::Config;
use crate::hook::{evaluate, home, Input, DATA_EXTS};
use dtk::lean::est_tokens;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Lines longer than this are skipped (a transcript line holds one whole tool result).
const MAX_LINE: usize = 32 * 1024 * 1024;
/// A result shorter than this cannot be a table worth replacing, so it is not even parsed.
const MIN_LINE: usize = 3000;
const PREVIEW: usize = 580;

#[derive(Default)]
struct Totals {
    files: usize,
    results: usize,
    bash: usize,
    read: usize,
    tables: usize,
    would_apply: usize,
    full_outputs: usize,
    full_saved: i64,
    saved_to_file: usize,
    saved_to_file_summary_tokens: usize,
    missing_saved_file: usize,
    by_format: BTreeMap<String, usize>,
    top: Vec<(i64, String)>,
    left_alone: Vec<String>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

/// Read results carry `cat -n` style line numbers (`   12\t...`). Take them off.
fn strip_line_numbers(s: &str) -> String {
    s.lines()
        .map(|l| match l.split_once('\t') {
            Some((n, rest)) if !n.is_empty() && n.trim().bytes().all(|b| b.is_ascii_digit()) => {
                rest
            }
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn scan_file(path: &Path, cfg: &Config, t: &mut Totals) {
    let Ok(f) = fs::File::open(path) else { return };
    let mut rd = BufReader::new(f);
    let mut line = String::new();
    // tool_use id -> (tool, subject, is a ranged read)
    let mut uses: HashMap<String, (String, String, bool)> = HashMap::new();
    t.files += 1;
    loop {
        line.clear();
        match rd.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(n) if n > MAX_LINE => continue,
            Ok(_) => {}
        }
        if line.contains("\"type\":\"tool_use\"") && !line.contains("\"type\":\"tool_result\"") {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                if b["type"] == "tool_use" {
                    let (name, input) = (b["name"].as_str().unwrap_or(""), &b["input"]);
                    let subject = input["command"]
                        .as_str()
                        .or_else(|| input["file_path"].as_str())
                        .unwrap_or("");
                    let ranged = input.get("offset").is_some() || input.get("limit").is_some();
                    if let Some(id) = b["id"].as_str() {
                        uses.insert(
                            id.to_string(),
                            (name.to_string(), subject.to_string(), ranged),
                        );
                    }
                }
            }
            continue;
        }
        // A short line is skipped unfinished, unless it is the stub for a saved-to-file output:
        // that line is short but points at a big file.
        if (line.len() < MIN_LINE && !line.contains("persisted-output"))
            || !line.contains("\"type\":\"tool_result\"")
        {
            // Still count it, cheaply, when it is a result.
            if line.contains("\"type\":\"tool_result\"") {
                t.results += 1;
            }
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        for b in v["message"]["content"].as_array().into_iter().flatten() {
            if b["type"] != "tool_result" {
                continue;
            }
            t.results += 1;
            let Some((tool, subject, ranged)) =
                b["tool_use_id"].as_str().and_then(|id| uses.get(id))
            else {
                continue;
            };
            let content = result_text(&b["content"]);
            let (text, persisted, file, original_tokens) = match tool.as_str() {
                "Bash" => {
                    t.bash += 1;
                    match content
                        .strip_prefix("<persisted-output>")
                        .and_then(|r| r.split_once("Full output saved to: "))
                    {
                        Some((_, rest)) => {
                            let p = PathBuf::from(rest.lines().next().unwrap_or("").trim());
                            match fs::read_to_string(&p) {
                                Ok(full) => (full, true, Some(p), PREVIEW),
                                Err(_) => {
                                    t.missing_saved_file += 1;
                                    continue;
                                }
                            }
                        }
                        None => {
                            let n = est_tokens(&content);
                            (content, false, None, n)
                        }
                    }
                }
                "Read" => {
                    t.read += 1;
                    let ext = Path::new(subject)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if *ranged || !DATA_EXTS.contains(&ext.as_str()) {
                        continue;
                    }
                    let n = est_tokens(&content);
                    (
                        strip_line_numbers(&content),
                        false,
                        Some(PathBuf::from(subject)),
                        n,
                    )
                }
                _ => continue,
            };
            let v = evaluate(
                &Input {
                    tool: if tool == "Read" { "Read" } else { "Bash" },
                    subject,
                    text: &text,
                    file: file.as_deref(),
                    persisted,
                },
                cfg,
                false,
            );
            let Some(found) = &v.detected else { continue };
            t.tables += 1;
            *t.by_format.entry(found.name.to_string()).or_default() += 1;
            if !v.applies {
                let what: String = subject
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(60)
                    .collect();
                t.left_alone.push(format!(
                    "{} tok  {} {} rows  {}  ({})",
                    v.tokens_full, found.name, found.rows, what, v.reason
                ));
                continue;
            }
            t.would_apply += 1;
            let saved = original_tokens as i64 - v.after as i64;
            if persisted {
                t.saved_to_file += 1;
                t.saved_to_file_summary_tokens += v.after;
            } else {
                t.full_outputs += 1;
                t.full_saved += saved;
            }
            let what: String = subject
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(70)
                .collect();
            t.top.push((
                saved,
                format!(
                    "{:>7} -> {:>5} tok  {} {} rows  {}{}",
                    original_tokens,
                    v.after,
                    found.name,
                    found.rows,
                    what,
                    if persisted {
                        "  (saved to a file; the preview is replaced)"
                    } else {
                        ""
                    }
                ),
            ));
        }
    }
}

/// `dtk discover [--all | --since DAYS] [--limit N] [--json]`
pub fn run(args: &[String]) -> i32 {
    let (mut days, mut limit, mut json_out) = (Some(30u64), 8usize, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--all" => days = None,
            "--since" => days = it.next().and_then(|v| v.parse().ok()).or(days),
            "--limit" => limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(limit),
            "--json" => json_out = true,
            other => {
                eprintln!("dtk discover: unknown option {other}\nusage: dtk discover [--all | --since DAYS] [--limit N] [--json]");
                return 2;
            }
        }
    }
    let Some(root) = home().map(|h| h.join(".claude/projects")) else {
        return 1;
    };
    let mut files = Vec::new();
    walk(&root, &mut files);
    let cutoff = days.map(|d| SystemTime::now() - Duration::from_secs(d * 86_400));
    files.retain(
        |f| match (cutoff, fs::metadata(f).and_then(|m| m.modified())) {
            (Some(c), Ok(m)) => m >= c,
            _ => true,
        },
    );
    let cfg = Config::load().0;
    let mut t = Totals::default();
    for f in &files {
        scan_file(f, &cfg, &mut t);
    }
    t.top.sort_by_key(|x| std::cmp::Reverse(x.0));
    if json_out {
        let v = json!({
            "sessions": t.files, "tool_results": t.results, "bash": t.bash, "read": t.read,
            "tables": t.tables, "by_format": t.by_format, "would_replace": t.would_apply,
            "shown_in_full": {"outputs": t.full_outputs, "est_tokens_saved": t.full_saved},
            "saved_to_file": {"outputs": t.saved_to_file, "summary_tokens": t.saved_to_file_summary_tokens},
            "saved_file_missing": t.missing_saved_file,
            "left_alone": t.left_alone,
            "top": t.top.iter().take(limit).map(|x| x.1.clone()).collect::<Vec<_>>(),
        });
        println!("{v}");
        return 0;
    }
    let window = days.map_or("all time".to_string(), |d| format!("last {d} days"));
    println!(
        "dtk discover ({window}): {} sessions, {} tool results; {} Bash and {} Read results were big enough to check",
        t.files, t.results, t.bash, t.read
    );
    let fmts: Vec<String> = t
        .by_format
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    println!(
        "tables found: {}{}",
        t.tables,
        if fmts.is_empty() {
            String::new()
        } else {
            format!(" ({})", fmts.join(", "))
        }
    );
    println!("dtk would replace {} of them:", t.would_apply);
    println!(
        "  shown in full by Claude Code: {} outputs, about {} est. tokens saved",
        t.full_outputs, t.full_saved
    );
    println!(
        "  saved to a file by Claude Code: {} outputs; each ~{PREVIEW}-token preview becomes a summary of all rows ({} est. tokens in total)",
        t.saved_to_file, t.saved_to_file_summary_tokens
    );
    if t.missing_saved_file > 0 {
        println!(
            "  ({} saved-to-file outputs are no longer on disk and were skipped)",
            t.missing_saved_file
        );
    }
    if !t.left_alone.is_empty() {
        println!("tables left alone ({}):", t.left_alone.len());
        for l in t.left_alone.iter().take(limit) {
            println!("  {l}");
        }
    }
    if !t.top.is_empty() {
        println!("biggest opportunities:");
        for (i, (_, l)) in t.top.iter().take(limit).enumerate() {
            println!("  {}. {l}", i + 1);
        }
    }
    0
}
