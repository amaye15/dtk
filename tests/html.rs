//! Web pages: the text of a fetched page, what it leaves out, when it applies, and `dtk html`.

use dtk::html::{looks_like_html, parse, summary};
use dtk::lean::est_tokens;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn page() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/pages/docs_rs_dtk.html"
    ))
    .unwrap()
}

/// The visible words of a page, worked out in the simplest way there is: walk the tags with a
/// stack, and count the words of text that are not inside a script, a style, the head, a menu
/// (`nav`) or an element that is hidden. Not dtk's code.
fn words_by_hand(html: &str) -> HashMap<String, usize> {
    const HIDES: [&str; 8] = [
        "script", "style", "head", "svg", "noscript", "nav", "template", "select",
    ];
    const VOID: [&str; 10] = [
        "br", "hr", "img", "input", "meta", "link", "area", "base", "col", "wbr",
    ];
    let mut counts = HashMap::new();
    let mut stack: Vec<(String, bool)> = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            add_words(&mut counts, rest, &stack);
            break;
        };
        add_words(&mut counts, &rest[..lt], &stack);
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            if let Some(at) = stack.iter().rposition(|(n, _)| *n == name) {
                stack.truncate(at);
            }
            continue;
        }
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty()
            || tag.starts_with('!')
            || VOID.contains(&name.as_str())
            || tag.ends_with('/')
        {
            continue;
        }
        let lower = tag.to_ascii_lowercase();
        let hidden = HIDES.contains(&name.as_str())
            || lower.contains(" hidden")
            || lower.contains("aria-hidden=\"true\"")
            || lower.contains("role=\"navigation\"")
            || lower.replace(' ', "").contains("display:none");
        if name == "script" || name == "style" {
            let close = format!("</{name}");
            let end = rest.to_ascii_lowercase().find(&close).unwrap_or(rest.len());
            rest = &rest[end..];
        }
        stack.push((name, hidden));
    }
    counts
}

fn add_words(counts: &mut HashMap<String, usize>, text: &str, stack: &[(String, bool)]) {
    if stack.iter().any(|(_, hidden)| *hidden) {
        return;
    }
    let text = dtk::html::decode_entities(text);
    for w in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        *counts.entry(w.to_lowercase()).or_default() += 1;
    }
}

#[test]
fn a_real_page_keeps_its_words_and_loses_its_markup() {
    let raw = page();
    assert!(looks_like_html(&raw));
    let p = parse(&raw);
    // The text view is plain text: its words need no tag stripping.
    let mut got: HashMap<String, usize> = HashMap::new();
    for w in p
        .text
        .join(" ")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        *got.entry(w.to_lowercase()).or_default() += 1;
    }
    let want = words_by_hand(&raw);
    let total: usize = want.values().sum();
    let missing: usize = want
        .iter()
        .map(|(w, c)| c.saturating_sub(*got.get(w).unwrap_or(&0)))
        .sum();
    assert!(total > 100, "{total} visible words");
    let mut lost: Vec<(&String, usize)> = want
        .iter()
        .map(|(w, c)| (w, c.saturating_sub(*got.get(w).unwrap_or(&0))))
        .filter(|(_, n)| *n > 0)
        .collect();
    lost.sort();
    assert_eq!(
        missing, 0,
        "visible words that the text view lost: {lost:?}"
    );
    let s = summary(&p, &raw, "abcdef012345", None, 2500);
    assert!(
        est_tokens(&s) * 5 < est_tokens(&raw),
        "{} -> {}",
        est_tokens(&raw),
        est_tokens(&s)
    );
    assert!(p.title.starts_with("dtk"), "{}", p.title);
}

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-html-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, args: &[&str], stdin: &str) -> (String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(args)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
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
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn bash_event(command: &str, stdout: &str) -> String {
    json!({"hook_event_name": "PostToolUse", "tool_name": "Bash",
           "tool_input": {"command": command},
           "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false, "isImage": false}})
    .to_string()
}

/// A page big enough to be worth converting: the real one, repeated as sections.
fn big_page() -> String {
    let raw = page();
    let body_at = raw.find("<body").unwrap();
    let close = raw.rfind("</body>").unwrap();
    let inner = &raw[body_at..close];
    format!(
        "{}{}{}",
        &raw[..close],
        inner.replace("<body", "<div"),
        &raw[close..]
    )
}

#[test]
fn a_fetched_page_becomes_its_text_and_other_commands_do_not() {
    let home = temp_home("hook");
    let page = big_page();
    assert!(est_tokens(&page) > 1200);
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &bash_event("curl -sL https://docs.rs/dtk/latest/dtk/", &page),
    );
    let v: Value = serde_json::from_str(&out).expect("a fetched page is converted");
    let s = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap();
    assert!(s.starts_with("[dtk] web page \"dtk"), "{s}");
    assert!(est_tokens(s) * 3 < est_tokens(&page));
    // The same page read as source, or asked for in rows, or with the setting off: not converted.
    for cmd in [
        "cat index.html",
        "sed -n '1,300p' index.html",
        "grep -n title index.html",
        "curl -s https://x.org | head -c 4000",
    ] {
        let (out, _) = dtk(&home, &["hook", "claude"], &bash_event(cmd, &page));
        assert_eq!(out, "", "`{cmd}`");
    }
    // A page with headers in front of it (`curl -i`).
    let with_headers = format!("HTTP/2 200\ncontent-type: text/html\n\n{page}");
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &bash_event("curl -si https://docs.rs/x", &with_headers),
    );
    let v: Value = serde_json::from_str(&out).expect("headers are allowed");
    assert!(v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap()
        .contains("response: HTTP/2 200"));
    // A small page is left as it is.
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &bash_event(
            "curl -s https://example.com",
            "<!doctype html><html><head><title>t</title></head><body><p>hello</p></body></html>",
        ),
    );
    assert_eq!(out, "");
    let cfg = home.join(".config/dtk");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[hooks]\nhtml_pages = false\n").unwrap();
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &bash_event("curl -sL https://docs.rs/dtk/latest/dtk/", &page),
    );
    assert_eq!(out, "", "html_pages = false");
}

#[test]
fn the_saved_page_can_be_read_again_part_by_part() {
    let home = temp_home("drill");
    let page = big_page();
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &bash_event("curl -sL https://docs.rs/dtk/latest/dtk/", &page),
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    let s = v["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
        .as_str()
        .unwrap();
    let id = s
        .split("saved as ")
        .nth(1)
        .unwrap()
        .split('.')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(id.len(), 12, "{id}");
    let (headings, _) = dtk(&home, &["html", &id, "headings"], "");
    assert!(headings.contains("(line "), "{headings}");
    let (text, _) = dtk(&home, &["html", &id, "text", "0", "5"], "");
    assert!(text.starts_with("1: "), "{text}");
    let (found, _) = dtk(&home, &["html", &id, "grep", "dtk"], "");
    assert!(found.contains("lines contain"), "{found}");
    let (links, _) = dtk(&home, &["html", &id, "links", "3"], "");
    assert!(links.contains(" -> "), "{links}");
    // dtk's own command is never summarized again.
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &bash_event(&format!("dtk html {id} text 0 3000"), &text.repeat(300)),
    );
    assert_eq!(out, "");
}

// ---- the Read tool and MCP tools --------------------------------------------------------------

fn read_event(path: &str, content: &str, ranged: bool) -> String {
    let lines = content.lines().count();
    let mut input = json!({"file_path": path});
    if ranged {
        input["offset"] = json!(1);
        input["limit"] = json!(500);
    }
    json!({"hook_event_name": "PostToolUse", "tool_name": "Read", "tool_input": input,
           "tool_response": {"type": "text", "file": {"filePath": path, "content": content,
               "numLines": lines, "startLine": 1, "totalLines": lines}}})
    .to_string()
}

fn mcp_event(tool: &str, response: Value) -> String {
    json!({"hook_event_name": "PostToolUse", "tool_name": tool, "tool_input": {},
           "tool_response": response, "mcp_server": {"name": "x", "source": "dynamic"}})
    .to_string()
}

#[test]
fn a_big_html_file_that_is_read_becomes_its_text_and_says_how_to_see_the_markup() {
    let home = temp_home("read");
    let page = big_page();
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &read_event("/work/report.html", &page, false),
    );
    let v: Value = serde_json::from_str(&out).expect("a big HTML file is converted");
    let file = &v["hookSpecificOutput"]["updatedToolOutput"]["file"];
    let s = file["content"].as_str().unwrap();
    assert!(s.starts_with("[dtk] web page \"dtk"), "{s}");
    assert!(
        s.contains("this is the text of the file /work/report.html, without its markup. To see or edit the markup, read it with offset and limit, or use cat."),
        "{s}"
    );
    assert_eq!(
        file["numLines"], file["totalLines"],
        "the counts describe what is shown"
    );
    assert!(est_tokens(s) * 3 < est_tokens(&page));
}

#[test]
fn an_html_file_that_is_read_for_its_markup_is_left_alone() {
    let home = temp_home("read-skip");
    let page = big_page();
    let quiet = |ev: String| {
        let (out, _) = dtk(&home, &["hook", "claude"], &ev);
        out
    };
    // A range is a request for exact lines.
    assert_eq!(quiet(read_event("/work/report.html", &page, true)), "");
    // A small file is source being worked on.
    let small = "<!doctype html><html><head><title>t</title></head><body><h1>Hello</h1><p>text</p></body></html>";
    assert_eq!(quiet(read_event("/work/index.html", small, false)), "");
    // A template: a program fills it in and a person edits it.
    for marker in [
        "{{ title }}",
        "{% block x %}",
        "<div v-if=\"a\">",
        "<?php echo 1; ?>",
        "<li *ngFor=\"let x of xs\">",
    ] {
        let templ = page.replacen("<body", &format!("<body data-x=\"{marker}\""), 1);
        assert_eq!(
            quiet(read_event("/work/base.html", &templ, false)),
            "",
            "{marker}"
        );
    }
    // Not an HTML file by its name.
    assert_eq!(quiet(read_event("/work/notes.md", &page, false)), "");
    // The setting is off.
    let cfg = home.join(".config/dtk");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[hooks]\nhtml_reads = false\n").unwrap();
    assert_eq!(quiet(read_event("/work/report.html", &page, false)), "");
}

#[test]
fn a_page_that_an_mcp_tool_returns_becomes_its_text_in_the_same_shape() {
    let home = temp_home("mcp");
    let page = big_page();
    // Content blocks stay blocks.
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &mcp_event("mcp__fetch__fetch", json!([{"type": "text", "text": page}])),
    );
    let v: Value = serde_json::from_str(&out).expect("a page from an MCP tool is converted");
    let upd = &v["hookSpecificOutput"]["updatedToolOutput"];
    assert_eq!(upd[0]["type"], "text");
    assert!(upd[0]["text"]
        .as_str()
        .unwrap()
        .starts_with("[dtk] web page"));
    // A string stays a string.
    let (out, _) = dtk(
        &home,
        &["hook", "claude"],
        &mcp_event("mcp__browser__page_html", json!(page)),
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["hookSpecificOutput"]["updatedToolOutput"]
        .as_str()
        .unwrap()
        .starts_with("[dtk] web page"));
}

#[test]
fn an_mcp_tool_that_reads_files_is_left_alone() {
    let home = temp_home("mcp-skip");
    let page = big_page();
    let quiet = |tool: &str, text: &str| {
        let (out, _) = dtk(
            &home,
            &["hook", "claude"],
            &mcp_event(tool, json!([{"type": "text", "text": text}])),
        );
        out
    };
    for tool in [
        "mcp__filesystem__read_file",
        "mcp__files__read_text_file",
        "mcp__repo__get_file_contents",
    ] {
        assert_eq!(quiet(tool, &page), "", "{tool} reads a file from the disk");
    }
    // A live page with `{{` in it (a code sample, a script) is still a page.
    let with_braces = page.replacen("<body", "<body data-x=\"{{ name }}\"", 1);
    assert!(quiet("mcp__fetch__fetch", &with_braces).contains("[dtk] web page"));
    let small = "<!doctype html><html><body><p>hi</p></body></html>";
    assert_eq!(quiet("mcp__fetch__fetch", small), "", "a small page");
    // exclude_commands names the tool.
    let cfg = home.join(".config/dtk");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[hooks]\nexclude_commands = [\"^mcp__fetch__\"]\n",
    )
    .unwrap();
    assert_eq!(quiet("mcp__fetch__fetch", &page), "");
}
