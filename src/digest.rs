//! A digest of a long log: build output, test output, a program's progress messages.
//!
//! A long log is mostly lines of a few kinds (`Compiling x v1.2.3`, `test a::b ... ok`) with a
//! few that matter among them (the errors, the warnings, the last line that says how it ended).
//! Claude Code saves an output of more than 30,000 characters to a file and shows the model the
//! first lines of it, which are almost always the boring ones. The digest shows what the whole
//! output is made of, and every kind of error and warning with the line number of the first one
//! and the lines that follow it. It is built by counting, with no model, so it is instant, it
//! costs nothing and it is the same every time. The full text stays in the saved file; line
//! numbers in the digest are the numbers of that file.
//!
//! This is for logs only. Source code, documents, diffs, lists of names and results of a search
//! are the lines themselves, and a digest would hide the very thing that was asked for. So the
//! text must look like a log in two ways: a few line shapes make up most of it, and most of its
//! lines say something a log says (a level, a status, a timestamp, `Compiling`, `test ... ok`).

use crate::lean::{est_tokens, template};
use std::collections::HashMap;

/// A log shorter than this is read as it is.
pub const MIN_LINES: usize = 150;
/// Characters shown of one line.
const LINE_CHARS: usize = 170;
/// Kinds of error and of warning listed.
const MAX_ERROR_KINDS: usize = 20;
const MAX_WARN_KINDS: usize = 10;

/// What a command is, for deciding whether its output can be a log. The output of these is the
/// content that was asked for: files, search results, listings, diffs, pages.
pub fn prints_content(command: &str) -> bool {
    const CONTENT: [&str; 36] = [
        "cat", "bat", "less", "more", "head", "tail", "sed", "awk", "grep", "egrep", "fgrep", "rg",
        "ag", "ack", "find", "fd", "ls", "tree", "diff", "cmp", "wc", "sort", "uniq", "cut", "tr",
        "xxd", "hexdump", "strings", "jq", "yq", "curl", "wget", "http", "locate", "du", "tac",
    ];
    const GIT: [&str; 9] = [
        "diff", "show", "log", "blame", "grep", "ls-files", "ls-tree", "cat-file", "shortlog",
    ];
    for segment in command.split(['|', ';', '&']) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let mut i = 0;
        // Skip `cd dir &&` style prefixes and NAME=value assignments.
        while i < words.len() && words[i].contains('=') && !words[i].starts_with('-') {
            i += 1;
        }
        let Some(prog) = words.get(i) else { continue };
        let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
        if CONTENT.contains(&base) {
            return true;
        }
        if base == "git" && words.iter().skip(i + 1).any(|w| GIT.contains(w)) {
            return true;
        }
    }
    false
}

/// Words that make a line a kind of problem.
const ERROR_WORDS: [&str; 17] = [
    "error",
    "errors",
    "fail",
    "failed",
    "failure",
    "failures",
    "panic",
    "panicked",
    "fatal",
    "exception",
    "traceback",
    "assertionerror",
    "segfault",
    "aborted",
    "cannot",
    "unable",
    "denied",
];
const WARN_WORDS: [&str; 4] = ["warn", "warning", "warnings", "deprecated"];

fn words(line: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in line
        .char_indices()
        .chain(std::iter::once((line.len(), ' ')))
    {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, line[s..i].to_ascii_lowercase()));
        }
    }
    out.into_iter()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Severity {
    Error,
    Warn,
}

/// The severity of a line, if any word in it names one. `0 failed` and `no errors` do not count.
fn severity(line: &str) -> Option<Severity> {
    let ws: Vec<(usize, String)> = words(line).collect();
    let mut found = None;
    for (k, (at, w)) in ws.iter().enumerate() {
        let kind = if ERROR_WORDS.contains(&w.as_str()) {
            Severity::Error
        } else if WARN_WORDS.contains(&w.as_str()) {
            Severity::Warn
        } else {
            continue;
        };
        // `error_handler`, `error-chain`, `src/error.rs`, `std::error::Error` are names, not
        // messages. A word is a message when it stands alone.
        let bytes = line.as_bytes();
        let end = *at + w.len();
        let glued_before = *at > 0 && b"_-./".contains(&bytes[*at - 1])
            || (*at > 1 && &bytes[*at - 2..*at] == b"::");
        let glued_after = end < bytes.len()
            && (b"_-./".contains(&bytes[end])
                || (bytes[end] == b':' && bytes.get(end + 1) == Some(&b':')));
        if glued_before || glued_after {
            continue;
        }
        let before = k.checked_sub(1).map(|p| ws[p].1.as_str());
        let after = ws.get(k + 1).map(|n| n.1.as_str());
        let number_after = line[*at + w.len()..]
            .trim_start_matches([':', '=', ' '])
            .starts_with('0')
            && !line[*at + w.len()..]
                .trim_start_matches([':', '=', ' '])
                .starts_with(|c: char| c.is_ascii_digit() && c != '0');
        if matches!(before, Some("0" | "no" | "zero" | "without"))
            || (number_after && after == Some("0"))
        {
            continue;
        }
        // An error wins over a warning in the same line.
        if kind == Severity::Error {
            return Some(Severity::Error);
        }
        found = Some(kind);
    }
    found
}

/// A line that begins as a diagnostic does: `error[E0425]: ...`, `warning: ...`, `ERROR ...`,
/// `FAILED tests/x.py::t`, `E   assert ...`, `src/a.c:12:5: error: ...`, `fatal: ...`.
fn begins_as_problem(line: &str) -> bool {
    let l = message(line).trim_start();
    let first: String = l
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    let after = l[first.len()..].chars().next();
    let head = matches!(
        first.as_str(),
        "error" | "warning" | "fatal" | "panic" | "failed" | "failure" | "exception" | "traceback"
    ) && matches!(after, Some(':' | '[' | ' ' | '(') | None);
    // `path:12:5: error: ...` (gcc, clang, javac, mypy) and `E   assert ...` (pytest).
    let located = [": error", ": warning", ": fatal error"]
        .iter()
        .any(|m| l.contains(m))
        && l.split(':')
            .nth(1)
            .is_some_and(|n| n.trim().chars().all(|c| c.is_ascii_digit()) && !n.trim().is_empty());
    let pytest = l.starts_with("E  ") && l[1..].trim_start().len() > 2;
    head || located || pytest
}

/// Does the line say something that a log says?
fn log_word(line: &str) -> bool {
    const MARKS: [&str; 24] = [
        "INFO",
        "WARN",
        "ERROR",
        "DEBUG",
        "TRACE",
        "FATAL",
        "PASS",
        "FAIL",
        "PASSED",
        "FAILED",
        "Compiling",
        "Downloading",
        "Downloaded",
        "Building",
        "Running",
        "Installing",
        "Collecting",
        "Finished",
        "Checking",
        "Fetching",
        "Resolved",
        "Installed",
        "SUCCESS",
        "BUILD",
    ];
    let l = line.trim_start();
    let timestamped = {
        let b = l.trim_start_matches(['[', '(']).as_bytes();
        (b.len() > 8 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit))
            || (b.len() > 8
                && b[2] == b':'
                && b[5] == b':'
                && b[..2].iter().all(u8::is_ascii_digit))
    };
    timestamped
        || l.starts_with("test ")
        || l.contains(" ... ok")
        || l.contains(" PASSED")
        || l.contains(" FAILED")
        || l.contains(" ... FAILED")
        || MARKS
            .iter()
            .any(|m| l.split(|c: char| !c.is_alphanumeric()).any(|w| w == *m))
}

/// A rough shape of a line for counting kinds: the exact shape of `template`, and every word that
/// looks like a name (`a::b`, `x_y`, `v1.2.3`, a path) made into `*`.
fn coarse(line: &str) -> String {
    let t = template(line.trim());
    let mut out: Vec<&str> = Vec::new();
    for w in t.split_whitespace() {
        let name_like = w.contains(['_', ':', '/', '\\'])
            || w.trim_end_matches(['.', ',']).contains('.')
            || w.chars().skip(1).any(|c| c.is_uppercase()) && w.chars().any(|c| c.is_lowercase());
        let w = if name_like { "*" } else { w };
        if !(w == "*" && out.last() == Some(&"*")) {
            out.push(w);
        }
    }
    out.join(" ")
}

/// `unused variable `x`` and `unused variable `y`` are one kind of warning: what is between
/// quotes or backticks is a name, not part of the message.
fn without_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, '`' | '"' | '\'') {
            // A short span between two of the same mark is a name.
            let end = chars[i + 1..].iter().take(60).position(|&x| x == c);
            if let Some(len) = end.filter(|&len| len > 0) {
                out.push(c);
                out.push('*');
                out.push(c);
                i += len + 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn timestamped(line: &str) -> bool {
    let b = line.trim_start().trim_start_matches(['[', '(']).as_bytes();
    (b.len() > 8 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit))
        || (b.len() > 8 && b[2] == b':' && b[5] == b':' && b[..2].iter().all(u8::is_ascii_digit))
}

/// A line that belongs to the one above it: indented, or one of the marks that compilers, test
/// runners and Python print under a message. A line of the log itself (it has a timestamp) does
/// not.
fn continues(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() || timestamped(line) {
        return false;
    }
    const MARKS: [&str; 13] = [
        "-->",
        "|",
        "=",
        "^",
        "at ",
        "Caused by",
        "E ",
        "note:",
        "help:",
        "thread '",
        "stack backtrace",
        "File \"",
        "Traceback",
    ];
    line.starts_with([' ', '\t']) || MARKS.iter().any(|m| t.starts_with(m))
}

/// The file a diagnostic points at: `--> src/a.rs:12:5` (rustc), `src/a.c:12:5: error: ...`
/// (gcc, clang, javac, mypy, tsc style).
fn located_file(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let spot = match t.strip_prefix("--> ") {
        Some(rest) => rest,
        None if t.contains(": error") || t.contains(": warning") => t,
        None => return None,
    };
    let file = spot.split(':').next()?;
    let numbered = spot[file.len()..]
        .strip_prefix(':')?
        .starts_with(|c: char| c.is_ascii_digit());
    (numbered && file.contains(['/', '\\', '.']) && !file.contains(' ')).then_some(file)
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// A leading timestamp and level are not part of what a message says.
fn message(line: &str) -> &str {
    let mut l = line.trim_start();
    for _ in 0..3 {
        let w_end = l.find(char::is_whitespace).unwrap_or(l.len());
        let w = &l[..w_end];
        let stamp = w.trim_matches(['[', ']', '(', ')']).as_bytes();
        let is_time = stamp.len() >= 5
            && (stamp[2] == b':' || (stamp.len() > 7 && stamp[4] == b'-' && stamp[7] == b'-'))
            && stamp[0].is_ascii_digit();
        if is_time {
            l = l[w_end..].trim_start();
        } else {
            break;
        }
    }
    l
}

/// Lines listed for one kind.
const LINES_PER_KIND: usize = 8;

struct Kind {
    first: usize,
    last: usize,
    at: Vec<usize>,
    count: usize,
    example: String,
    severity: Severity,
}

/// The digest of `text`, or `None` when it is not a log. `saved` is the file that holds the
/// text, when there is one.
pub fn digest(text: &str, saved: Option<&str>) -> Option<String> {
    let cleaned = crate::table::clean(text);
    let lines: Vec<&str> = cleaned.lines().collect();
    let nonblank: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].trim().is_empty())
        .collect();
    if nonblank.len() < MIN_LINES {
        return None;
    }
    let n = nonblank.len();

    // 1. What the log is made of.
    let mut shapes: HashMap<String, (usize, usize)> = HashMap::new();
    for &i in &nonblank {
        let e = shapes.entry(coarse(lines[i])).or_insert((0, i));
        e.0 += 1;
    }
    let mut by_count: Vec<(&String, &(usize, usize))> = shapes.iter().collect();
    by_count.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.1 .1.cmp(&b.1 .1)));
    let top: usize = by_count.iter().take(8).map(|(_, v)| v.0).sum();
    // 2. It reads like a log: a few shapes and log words; or it is a pile of diagnostics, where
    //    each message is a block of lines and only the first says "error".
    let logish = nonblank.iter().filter(|&&i| log_word(lines[i])).count();
    let a_log = top * 100 >= n * 60 && logish * 100 >= n * 30;
    // Diagnostics are blocks of lines that each begin with the severity word; it does not just
    // occur in them (source code and prose use "error" in the middle of a line all the time).
    let begun = nonblank
        .iter()
        .filter(|&&i| begins_as_problem(lines[i]))
        .count();
    let diagnostics = begun >= 5 && begun * 100 >= n * 2;
    if !(a_log || diagnostics) {
        return None;
    }

    // 3. The lines that matter, by kind.
    let mut kinds: HashMap<String, Kind> = HashMap::new();
    for &i in &nonblank {
        let Some(sev) = severity(lines[i]) else {
            continue;
        };
        let key = template(&without_quoted(message(lines[i])));
        kinds
            .entry(key)
            .and_modify(|k| {
                k.count += 1;
                k.last = i;
                if k.at.len() < LINES_PER_KIND {
                    k.at.push(i);
                }
            })
            .or_insert(Kind {
                first: i,
                last: i,
                at: vec![i],
                count: 1,
                example: clip(lines[i], LINE_CHARS),
                severity: sev,
            });
    }
    let pick = |sev: Severity, max: usize| {
        let mut v: Vec<&Kind> = kinds.values().filter(|k| k.severity == sev).collect();
        v.sort_by_key(|k| k.first);
        let total = v.len();
        v.truncate(max);
        (v, total)
    };
    let (errors, error_kinds) = pick(Severity::Error, MAX_ERROR_KINDS);
    let (warnings, warn_kinds) = pick(Severity::Warn, MAX_WARN_KINDS);
    let error_lines: usize = kinds
        .values()
        .filter(|k| k.severity == Severity::Error)
        .map(|k| k.count)
        .sum();
    let warn_lines: usize = kinds
        .values()
        .filter(|k| k.severity == Severity::Warn)
        .map(|k| k.count)
        .sum();

    // 4. Write it.
    let bytes = text.len();
    let mut out = String::new();
    out.push_str(&format!(
        "[dtk] log digest: {} lines, {}, about {} tokens. Line numbers below are those of the full output",
        lines.len(),
        crate::lean::human_bytes(bytes),
        est_tokens(text)
    ));
    match saved {
        Some(p) => out.push_str(&format!(", saved at {p}\n")),
        None => out.push('\n'),
    }
    out.push_str("made of:\n");
    // The bulk: the lines that are not problems (those are listed below).
    let is_problem = |i: usize| severity(lines[i]).is_some();
    let mut shown = 0;
    for (shape, (count, first)) in by_count.iter().filter(|(_, v)| !is_problem(v.1)).take(5) {
        out.push_str(&format!(
            "  {count:>6} x {}  (from line {})\n",
            clip(shape, 90),
            first + 1
        ));
        shown += count;
    }
    let problems = nonblank.iter().filter(|&&i| is_problem(i)).count();
    let rest = n.saturating_sub(shown + problems);
    if rest > 0 {
        out.push_str(&format!("  {rest:>6} other lines\n"));
    }
    out.push_str(&format!(
        "{} error {}, {} warning {}\n",
        error_lines,
        if error_lines == 1 { "line" } else { "lines" },
        warn_lines,
        if warn_lines == 1 { "line" } else { "lines" }
    ));
    let list = |out: &mut String, title: &str, items: &[&Kind], total: usize, context: bool| {
        if items.is_empty() {
            return;
        }
        out.push_str(&format!("{title} ({total} kinds):\n"));
        for k in items {
            let repeat = if k.count > 1 {
                let at: Vec<String> = k.at.iter().skip(1).map(|i| (i + 1).to_string()).collect();
                let more = if k.count > k.at.len() {
                    format!(" ... last {}", k.last + 1)
                } else {
                    String::new()
                };
                format!("  (x{}; also at lines {}{more})", k.count, at.join(", "))
            } else {
                String::new()
            };
            out.push_str(&format!("  line {}: {}{repeat}\n", k.first + 1, k.example));
            if context {
                // rustc, javac and compilers put the place and the code under the message.
                for l in lines
                    .iter()
                    .skip(k.first + 1)
                    .take_while(|l| continues(l))
                    .take(3)
                {
                    out.push_str(&format!("      {}\n", clip(l, 130)));
                }
            }
        }
        if total > items.len() {
            out.push_str(&format!("  ... {} more kinds\n", total - items.len()));
        }
    };
    list(&mut out, "errors", &errors, error_kinds, true);
    list(&mut out, "warnings", &warnings, warn_kinds, false);
    // Where the problems are, when the log points at files.
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for &i in &nonblank {
        if let Some(f) = located_file(lines[i]) {
            *by_file.entry(f).or_default() += 1;
        }
    }
    if by_file.values().sum::<usize>() >= 3 {
        let mut v: Vec<(&&str, &usize)> = by_file.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let shown: Vec<String> = v.iter().take(8).map(|(f, c)| format!("{f} {c}")).collect();
        let more = if v.len() > 8 {
            format!(", +{} more files", v.len() - 8)
        } else {
            String::new()
        };
        out.push_str(&format!("in files: {}{more}\n", shown.join(", ")));
    }
    out.push_str("first lines:\n");
    for &i in nonblank.iter().take(3) {
        out.push_str(&format!("  {}: {}\n", i + 1, clip(lines[i], LINE_CHARS)));
    }
    // Test runners and build tools say how it ended, and list what failed, at the end.
    out.push_str("last lines:\n");
    for &i in nonblank.iter().skip(n.saturating_sub(12)) {
        out.push_str(&format!("  {}: {}\n", i + 1, clip(lines[i], 130)));
    }
    if let Some(p) = saved {
        out.push_str(&format!(
            "read more: sed -n '120,160p' {p}   |   grep -n PATTERN {p}\n"
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_log() -> String {
        let mut lines = Vec::new();
        for i in 0..400 {
            lines.push(format!("   Compiling crate_{i} v1.{}.0", i % 9));
        }
        lines.insert(120, "warning: unused variable `x`".to_string());
        lines.insert(121, "  --> src/lib.rs:12:9".to_string());
        lines.insert(
            300,
            "error[E0277]: the trait bound `Foo: Bar` is not satisfied".to_string(),
        );
        lines.insert(301, "  --> src/main.rs:40:5".to_string());
        lines.insert(302, "   |".to_string());
        lines.insert(303, "40 |     use_it(foo);".to_string());
        lines.push(
            "error: could not compile `app` (bin \"app\") due to 1 previous error".to_string(),
        );
        lines.push("test result: FAILED. 12 passed; 0 failed; 1 ignored".to_string());
        lines.join("\n") + "\n"
    }

    #[test]
    fn a_build_log_is_made_of_a_few_shapes_and_a_few_problems() {
        let d = digest(&build_log(), Some("/tmp/out.txt")).unwrap();
        assert!(d.contains("log digest: 408 lines"), "{d}");
        assert!(d.contains("400 x Compiling *"), "{d}");
        // The first error with the place under it, and the line number in the full output.
        assert!(d.contains("line 301: error[E0277]: the trait bound"), "{d}");
        assert!(d.contains("--> src/main.rs:40:5"), "{d}");
        assert!(d.contains("line 121: warning: unused variable"), "{d}");
        // `0 failed` does not make a line an error.
        assert!(d.contains("sed -n"), "{d}");
        assert!(est_tokens(&d) < 900, "{} tokens", est_tokens(&d));
    }

    #[test]
    fn code_prose_and_lists_are_not_logs() {
        let code: String = (0..300)
            .map(|i| format!("    let value_{i} = compute(input_{i}, {});\n", i * 7))
            .collect();
        assert!(digest(&code, None).is_none(), "source code");
        let prose: String = (0..300)
            .map(|i| {
                format!(
                    "Sentence number {i} says something different about topic {}.\n",
                    i % 17
                )
            })
            .collect();
        assert!(digest(&prose, None).is_none(), "prose");
        let names: String = (0..400)
            .map(|i| format!("./src/module_{i}/file_{}.rs\n", i % 13))
            .collect();
        assert!(
            digest(&names, None).is_none(),
            "a list of paths has no log words"
        );
    }

    #[test]
    fn short_logs_are_left_alone() {
        let short: String = (0..100).map(|i| format!("INFO step {i} done\n")).collect();
        assert!(digest(&short, None).is_none());
    }

    #[test]
    fn commands_that_print_content_never_get_a_digest() {
        for c in [
            "cat build.log",
            "cd x && grep -rn error src",
            "git diff HEAD~3",
            "find . -name '*.log'",
            "curl -s https://example.com/log.txt",
            "FOO=1 tail -n 5000 out.log",
            "ls -la | sort",
        ] {
            assert!(prints_content(c), "{c}");
        }
        for c in [
            "cargo test",
            "cd app && npm install",
            "uv run pytest -v",
            "make -j8",
            "git status",
        ] {
            assert!(!prints_content(c), "{c}");
        }
    }

    #[test]
    fn counts_and_negations_are_not_problems() {
        assert_eq!(
            severity("test result: ok. 12 passed; 0 failed; 0 ignored"),
            None
        );
        assert_eq!(severity("Finished in 3s with no errors"), None);
        assert_eq!(severity("errors: 0, warnings: 0"), None);
        assert_eq!(severity("1 failed, 4 passed"), Some(Severity::Error));
        assert_eq!(severity("warning: unused import"), Some(Severity::Warn));
        assert_eq!(severity("warning: this is an error"), Some(Severity::Error));
    }
}
