//! HTML pages: the visible text of a page fetched from the web, without the markup.
//!
//! A page is mostly markup. Wikipedia's article on CSV is 48,000 tokens of HTML and 4,400 tokens
//! of text; the Python documentation page is 25,000 and 5,700. The model asked for the content,
//! so it gets the content: headings, paragraphs, lists, tables and links (with their addresses),
//! in reading order. Scripts, styles, SVG drawings, hidden elements and the page's navigation
//! menu are left out, and the summary says so. The raw HTML is kept on disk, and
//! `dtk html ID text|headings|links|tables|json|grep` reads any part of it again.
//!
//! This is for pages that were fetched to be read (`curl`, `wget`). `cat page.html` is a person
//! working on the markup itself, and there the markup is the point; that is never converted.
//!
//! The reader is a small tolerant tokenizer, not a full HTML parser: real pages are broken in
//! ways a strict one refuses.

use crate::lean::est_tokens;
use serde_json::Value;

/// Elements whose content is not text for a reader.
const SKIPPED: [&str; 13] = [
    "script", "style", "noscript", "svg", "template", "iframe", "object", "embed", "canvas",
    "select", "datalist", "head", "math",
];
const VOID: [&str; 14] = [
    "br", "hr", "img", "input", "meta", "link", "area", "base", "col", "embed", "source", "track",
    "wbr", "param",
];
const BLOCKS: [&str; 30] = [
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "aside",
    "ul",
    "ol",
    "li",
    "table",
    "tr",
    "blockquote",
    "form",
    "fieldset",
    "dl",
    "dt",
    "dd",
    "figure",
    "figcaption",
    "details",
    "summary",
    "address",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Text(String),
    Open {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Close(String),
    /// The content of a `<script>` or `<style>`, which is not markup.
    Raw {
        name: String,
        attrs: Vec<(String, String)>,
        content: String,
    },
}

/// An embedded block of JSON (`application/json`, `ld+json`, `__NEXT_DATA__`).
#[derive(Debug, Clone)]
pub struct Blob {
    pub kind: String,
    pub bytes: usize,
    pub content: String,
}

#[derive(Debug, Default, Clone)]
pub struct Counts {
    pub scripts: usize,
    pub script_bytes: usize,
    pub styles: usize,
    pub images: usize,
    pub forms: usize,
    pub frames: usize,
    pub nav_links: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Page {
    pub title: String,
    pub description: String,
    /// The visible text, one entry per line, in reading order.
    pub text: Vec<String>,
    /// (text, address)
    pub links: Vec<(String, String)>,
    pub tables: Vec<Vec<Vec<String>>>,
    /// (level, text, index into `text`)
    pub headings: Vec<(usize, String, usize)>,
    pub data: Vec<Blob>,
    pub counts: Counts,
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let name = &tail[..e];
            let ch = if let Some(num) = name.strip_prefix('#') {
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse::<u32>().ok(),
                };
                code.and_then(char::from_u32)
            } else {
                named_entity(name)
            };
            ch.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &tail[e + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "times" => '×',
        "divide" => '÷',
        "deg" => '°',
        "plusmn" => '±',
        "frac12" => '½',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "para" => '¶',
        "sect" => '§',
        _ => return None,
    })
}

/// Does the text look like a whole HTML page, possibly after the headers of an HTTP response?
pub fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(4096)
        .collect::<String>()
        .to_ascii_lowercase();
    let at = head.find("<!doctype html").or_else(|| head.find("<html"));
    let Some(at) = at else { return false };
    // Only a response's headers may come before it.
    let before = head[..at].trim();
    (before.is_empty() || before.starts_with("http/"))
        && text.to_ascii_lowercase().contains("<body")
}

/// A command that fetches a page from the web, or that has an address in it: the page was asked
/// for to be read. `cat page.html` and its kind are people working on the markup, and are not.
pub fn fetches_page(command: &str) -> bool {
    const FETCHERS: [&str; 8] = [
        "curl", "wget", "http", "https", "xh", "lynx", "w3m", "links",
    ];
    const SOURCE: [&str; 9] = [
        "cat", "bat", "less", "more", "head", "tail", "sed", "awk", "grep",
    ];
    let mut fetcher = false;
    for segment in command.split(['|', ';', '&']) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let mut i = 0;
        while i < words.len() && words[i].contains('=') && !words[i].starts_with('-') {
            i += 1;
        }
        let Some(prog) = words.get(i) else { continue };
        let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
        if SOURCE.contains(&base) {
            return false;
        }
        fetcher |= FETCHERS.contains(&base);
    }
    fetcher || command.contains("http://") || command.contains("https://")
}

/// The headers of an HTTP response (`curl -i`), when the text starts with them: the status line,
/// and the page that follows.
pub fn split_response(text: &str) -> (Option<&str>, &str) {
    if !text.starts_with("HTTP/") {
        return (None, text);
    }
    let status = text.lines().next();
    let body_at = text
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| text.find("\n\n").map(|i| i + 2))
        .unwrap_or(0);
    (status, &text[body_at..])
}

fn tokenize(html: &str) -> Vec<Tok> {
    let b = html.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut text_start = 0;
    let flush = |toks: &mut Vec<Tok>, from: usize, to: usize| {
        if to > from {
            toks.push(Tok::Text(html[from..to].to_string()));
        }
    };
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            flush(&mut toks, text_start, i);
            i = rest.find("-->").map_or(b.len(), |e| i + e + 3);
            text_start = i;
            continue;
        }
        let next = b.get(i + 1).copied().unwrap_or(0);
        if !(next.is_ascii_alphabetic() || next == b'/' || next == b'!' || next == b'?') {
            i += 1;
            continue;
        }
        // A tag runs to the next `>` outside quotes.
        let mut j = i + 1;
        let mut quote = 0u8;
        while j < b.len() {
            match (quote, b[j]) {
                (0, b'>') => break,
                (0, q @ (b'"' | b'\'')) => quote = q,
                (q, c) if q != 0 && c == q => quote = 0,
                _ => {}
            }
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        flush(&mut toks, text_start, i);
        let inner = &html[i + 1..j];
        i = j + 1;
        text_start = i;
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            toks.push(Tok::Close(name.trim().to_ascii_lowercase()));
            continue;
        }
        let (name, attrs) = parse_tag(inner.trim_end_matches('/'));
        if name.is_empty() {
            continue;
        }
        if matches!(name.as_str(), "script" | "style") && !inner.ends_with('/') {
            let lower = html[i..].to_ascii_lowercase();
            let close = lower.find(&format!("</{name}")).unwrap_or(lower.len());
            toks.push(Tok::Raw {
                name: name.clone(),
                attrs,
                content: html[i..i + close].to_string(),
            });
            i += close;
            text_start = i;
            continue;
        }
        toks.push(Tok::Open { name, attrs });
    }
    flush(&mut toks, text_start, b.len());
    toks
}

fn parse_tag(inner: &str) -> (String, Vec<(String, String)>) {
    let inner = inner.trim();
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let name = inner[..name_end].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut rest = inner[name_end..].trim_start();
    while !rest.is_empty() {
        let key_end = rest
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(rest.len());
        let key = rest[..key_end].to_ascii_lowercase();
        rest = rest[key_end..].trim_start();
        let mut value = String::new();
        if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start();
            if let Some(q) = r.chars().next().filter(|c| matches!(c, '"' | '\'')) {
                let body = &r[1..];
                let end = body.find(q).unwrap_or(body.len());
                value = body[..end].to_string();
                rest = body.get(end + 1..).unwrap_or("").trim_start();
            } else {
                let end = r.find(char::is_whitespace).unwrap_or(r.len());
                value = r[..end].to_string();
                rest = r[end..].trim_start();
            }
        }
        if !key.is_empty() {
            attrs.push((key, decode_entities(&value)));
        }
    }
    (name, attrs)
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn hidden(attrs: &[(String, String)]) -> bool {
    attr(attrs, "hidden").is_some()
        || attr(attrs, "aria-hidden") == Some("true")
        || attr(attrs, "style").is_some_and(|s| {
            let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            s.contains("display:none") || s.contains("visibility:hidden")
        })
}

struct Render {
    page: Page,
    lines: Vec<String>,
    cur: String,
    /// Open elements, and whether each one hides what is inside it.
    stack: Vec<(String, bool)>,
    skip: usize,
    nav_depth: Option<usize>,
    in_title: bool,
    in_pre: bool,
    lists: Vec<Option<usize>>,
    heading: Option<(usize, usize)>,
    link: Option<(usize, String)>,
    table: Option<TableState>,
    table_depth: usize,
    /// For each `<table>` in the page, in order: does it hold another table? Then it lays the
    /// page out and is not data, and its content is read as ordinary text.
    layout_tables: Vec<bool>,
    next_table: usize,
    table_kinds: Vec<bool>,
}

struct TableState {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: Option<String>,
}

impl Render {
    fn push_line(&mut self, line: String) {
        self.lines.push(line);
    }

    fn block(&mut self) {
        let line = self.cur.trim_end().to_string();
        self.link = None;
        if !line.trim().is_empty() {
            self.push_line(line);
        }
        self.cur.clear();
    }

    fn blank(&mut self) {
        self.block();
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.push_line(String::new());
        }
    }

    fn text(&mut self, raw: &str) {
        if self.in_title {
            self.page.title.push_str(&decode_entities(raw));
            return;
        }
        if self.skip > 0 {
            return;
        }
        let s = decode_entities(raw);
        if let Some(t) = self.table.as_mut().filter(|_| self.table_depth >= 1) {
            if let Some(cell) = t.cell.as_mut() {
                append_words(cell, &s);
                return;
            }
        }
        if self.in_pre {
            self.cur.push_str(&s);
            return;
        }
        append_words(&mut self.cur, &s);
    }

    fn open(&mut self, name: &str, attrs: &[(String, String)]) {
        let is_void = VOID.contains(&name);
        let hides = hidden(attrs)
            || SKIPPED.contains(&name)
            || (name == "nav" || attr(attrs, "role") == Some("navigation"));
        if name == "title" {
            self.in_title = true;
        }
        if name == "meta" && self.page.description.is_empty() {
            let key = attr(attrs, "name")
                .or(attr(attrs, "property"))
                .unwrap_or("");
            if matches!(key, "description" | "og:description") {
                self.page.description = decode_entities(attr(attrs, "content").unwrap_or(""))
                    .trim()
                    .to_string();
            }
        }
        match name {
            "img" => self.page.counts.images += 1,
            "form" => self.page.counts.forms += 1,
            "iframe" | "frame" => self.page.counts.frames += 1,
            _ => {}
        }
        if is_void {
            if self.skip == 0 && !hides {
                self.void_element(name, attrs);
            }
            return;
        }
        if hides && self.skip == 0 && (name == "nav" || attr(attrs, "role") == Some("navigation")) {
            self.nav_depth = Some(self.stack.len());
        }
        self.stack.push((name.to_string(), hides));
        if hides {
            self.skip += 1;
            return;
        }
        if self.skip > 0 {
            if name == "a" && self.nav_depth.is_some() && attr(attrs, "href").is_some() {
                self.page.counts.nav_links += 1;
            }
            return;
        }
        self.start_element(name, attrs);
    }

    fn void_element(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "br" => {
                if self.table.is_none() {
                    self.block();
                }
            }
            "hr" => {
                self.blank();
                self.push_line("---".into());
                self.blank();
            }
            "img" => {
                let alt = attr(attrs, "alt").unwrap_or("").trim();
                if !alt.is_empty() && alt.chars().count() <= 100 {
                    self.text(&format!(" ![{alt}] "));
                }
            }
            _ => {}
        }
    }

    fn start_element(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if self.table_depth == 0 => {
                self.blank();
                let level = (name.as_bytes()[1] - b'0') as usize;
                self.cur.push_str(&"#".repeat(level));
                self.cur.push(' ');
                self.heading = Some((level, self.cur.len()));
            }
            "ul" | "ol" => {
                if self.table_depth == 0 {
                    self.block();
                }
                self.lists.push(if name == "ol" { Some(0) } else { None });
            }
            "li" if self.table_depth == 0 => {
                self.block();
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{n}. ")
                    }
                    _ => "- ".to_string(),
                };
                self.cur = format!("{}{marker}", "  ".repeat(depth));
            }
            "pre" => {
                self.blank();
                self.in_pre = true;
            }
            "a" => {
                if let Some(href) = attr(attrs, "href") {
                    let h = href.trim();
                    if !(h.is_empty() || h.starts_with('#') || h.starts_with("javascript:")) {
                        let target = if self.table_depth >= 1 {
                            self.table
                                .as_ref()
                                .and_then(|t| t.cell.as_ref())
                                .map_or(0, String::len)
                        } else {
                            self.cur.len()
                        };
                        self.link = Some((target, h.chars().take(300).collect()));
                    }
                }
            }
            // A button is its own box on the page: it does not run into the text beside it.
            "button" | "label" if self.table_depth == 0 => {
                if !self.cur.is_empty() && !self.cur.ends_with(' ') {
                    self.cur.push(' ');
                }
            }
            "table" => {
                let layout = self
                    .layout_tables
                    .get(self.next_table)
                    .copied()
                    .unwrap_or(false);
                self.next_table += 1;
                self.table_kinds.push(layout);
                if layout {
                    self.block();
                } else {
                    self.table_depth += 1;
                    if self.table_depth == 1 {
                        self.blank();
                        self.table = Some(TableState {
                            rows: Vec::new(),
                            row: Vec::new(),
                            cell: None,
                        });
                    }
                }
            }
            "td" | "th" if self.table_depth == 0 => {
                if !self.cur.trim().is_empty() && !self.cur.ends_with("| ") {
                    self.cur.push_str(" | ");
                }
            }
            "tr" if self.table_depth == 1 => {
                if let Some(t) = self.table.as_mut() {
                    t.row.clear();
                }
            }
            "td" | "th" if self.table_depth == 1 => {
                if let Some(t) = self.table.as_mut() {
                    t.cell = Some(String::new());
                }
            }
            n if BLOCKS.contains(&n) && self.table_depth == 0 => {
                if matches!(n, "p" | "blockquote" | "figure" | "details") {
                    self.blank();
                } else {
                    self.block();
                }
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        // Find the element; real pages leave tags open and close them out of order.
        let Some(pos) = self.stack.iter().rposition(|(n, _)| n == name) else {
            return;
        };
        let popped: Vec<(String, bool)> = self.stack.drain(pos..).collect();
        for (n, hid) in popped.iter().rev() {
            if *hid {
                self.skip -= 1;
            }
            if n == "title" {
                self.in_title = false;
            }
            if self.nav_depth == Some(pos) && (n == "nav" || *hid) && self.skip == 0 {
                let links = std::mem::take(&mut self.page.counts.nav_links);
                self.nav_depth = None;
                if links > 0 {
                    self.block();
                    self.push_line(format!("(navigation menu: {links} links)"));
                }
            }
            if !*hid && self.skip == 0 {
                self.end_element(n);
            }
        }
    }

    fn end_element(&mut self, name: &str) {
        match name {
            "a" => {
                if let Some((start, href)) = self.link.take() {
                    let in_cell = self.table_depth >= 1;
                    let buf: &mut String = if in_cell {
                        match self.table.as_mut().and_then(|t| t.cell.as_mut()) {
                            Some(c) => c,
                            None => return,
                        }
                    } else {
                        &mut self.cur
                    };
                    if start <= buf.len() && buf.is_char_boundary(start) {
                        let label = buf[start..].trim().to_string();
                        if !label.is_empty() {
                            buf.truncate(start);
                            let lead = if buf.ends_with(' ') || buf.is_empty() {
                                ""
                            } else {
                                " "
                            };
                            buf.push_str(&format!("{lead}[{label}]({href})"));
                            self.page.links.push((label, href));
                        }
                    }
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if self.table_depth == 0 => {
                if let Some((level, start)) = self.heading.take() {
                    let label = self.cur.get(start..).unwrap_or("").trim().to_string();
                    if label.is_empty() {
                        self.cur.clear();
                    } else {
                        self.page.headings.push((level, label, self.lines.len()));
                        self.block();
                    }
                }
                self.blank();
            }
            "ul" | "ol" => {
                self.lists.pop();
                if self.table_depth == 0 {
                    self.block();
                    if self.lists.is_empty() {
                        self.blank();
                    }
                }
            }
            "pre" => {
                let body: Vec<String> = self
                    .cur
                    .split('\n')
                    .map(|l| l.trim_end().to_string())
                    .collect();
                self.cur.clear();
                self.in_pre = false;
                let body: Vec<String> = body
                    .into_iter()
                    .skip_while(|l| l.is_empty())
                    .collect::<Vec<_>>();
                if !body.iter().all(|l| l.is_empty()) {
                    self.push_line("```".into());
                    for l in body {
                        self.push_line(l);
                    }
                    while self.lines.last().is_some_and(|l| l.is_empty()) {
                        self.lines.pop();
                    }
                    self.push_line("```".into());
                    self.push_line(String::new());
                }
            }
            "td" | "th" if self.table_depth == 1 => {
                if let Some(t) = self.table.as_mut() {
                    if let Some(c) = t.cell.take() {
                        t.row.push(c.trim().to_string());
                    }
                }
            }
            "tr" if self.table_depth == 1 => {
                if let Some(t) = self.table.as_mut() {
                    if let Some(c) = t.cell.take() {
                        t.row.push(c.trim().to_string());
                    }
                    if t.row.iter().any(|c| !c.is_empty()) {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
            }
            "table" => {
                let layout = self.table_kinds.pop().unwrap_or(false);
                if layout {
                    self.block();
                } else {
                    self.table_depth = self.table_depth.saturating_sub(1);
                    if self.table_depth == 0 {
                        if let Some(t) = self.table.take() {
                            self.finish_table(t.rows);
                        }
                    }
                }
            }
            n if BLOCKS.contains(&n) && self.table_depth == 0 => {
                if matches!(n, "p" | "blockquote" | "figure" | "details") {
                    self.blank();
                } else {
                    self.block();
                }
            }
            _ => {}
        }
    }

    fn finish_table(&mut self, rows: Vec<Vec<String>>) {
        if rows.is_empty() {
            return;
        }
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        for r in &rows {
            let line = r.join(" | ");
            if !line.trim_matches(|c| c == ' ' || c == '|').is_empty() {
                self.push_line(format!("| {line} |"));
            }
        }
        self.push_line(String::new());
        if rows.len() >= 2 && width >= 2 {
            self.page.tables.push(rows);
        }
    }
}

/// For each `<table>` in order of appearance: does another table sit inside it?
fn layout_tables(toks: &[Tok]) -> Vec<bool> {
    let mut layout: Vec<bool> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for t in toks {
        match t {
            Tok::Open { name, .. } if name == "table" => {
                for &outer in &open {
                    layout[outer] = true;
                }
                layout.push(false);
                open.push(layout.len() - 1);
            }
            Tok::Close(name) if name == "table" => {
                open.pop();
            }
            _ => {}
        }
    }
    layout
}

fn append_words(buf: &mut String, s: &str) {
    // A space between two inline elements (`<span>class</span><span> </span><span>csv</span>`).
    if !s.is_empty() && s.trim().is_empty() {
        if !buf.is_empty() && !buf.ends_with(' ') && !buf.ends_with('\n') {
            buf.push(' ');
        }
        return;
    }
    for (k, w) in s.split_whitespace().enumerate() {
        let starts_with_space = k == 0 && s.starts_with(char::is_whitespace);
        if (k > 0 || starts_with_space)
            && !buf.is_empty()
            && !buf.ends_with(' ')
            && !buf.ends_with('\n')
        {
            buf.push(' ');
        }
        buf.push_str(w);
    }
    if s.ends_with(char::is_whitespace) && !s.trim().is_empty() && !buf.ends_with(' ') {
        buf.push(' ');
    }
}

/// The page, read.
pub fn parse(html: &str) -> Page {
    let mut r = Render {
        page: Page::default(),
        lines: Vec::new(),
        cur: String::new(),
        stack: Vec::new(),
        skip: 0,
        nav_depth: None,
        in_title: false,
        in_pre: false,
        lists: Vec::new(),
        heading: None,
        link: None,
        table: None,
        table_depth: 0,
        layout_tables: Vec::new(),
        next_table: 0,
        table_kinds: Vec::new(),
    };
    let toks = tokenize(html);
    r.layout_tables = layout_tables(&toks);
    for tok in toks {
        match tok {
            Tok::Text(t) => r.text(&t),
            Tok::Open { name, attrs } => r.open(&name, &attrs),
            Tok::Close(name) => r.close(&name),
            Tok::Raw {
                name,
                attrs,
                content,
            } => {
                if name == "script" {
                    r.page.counts.scripts += 1;
                    r.page.counts.script_bytes += content.len();
                    let kind = attr(&attrs, "type").unwrap_or("").to_ascii_lowercase();
                    let id = attr(&attrs, "id").unwrap_or("");
                    let is_data = kind.contains("json") || id == "__NEXT_DATA__";
                    if is_data && serde_json::from_str::<Value>(content.trim()).is_ok() {
                        r.page.data.push(Blob {
                            kind: if id.is_empty() { kind } else { id.to_string() },
                            bytes: content.len(),
                            content: content.trim().to_string(),
                        });
                    }
                } else {
                    r.page.counts.styles += 1;
                }
            }
        }
    }
    r.blank();
    let mut page = r.page;
    page.title = page.title.split_whitespace().collect::<Vec<_>>().join(" ");
    // One blank line between blocks, none at the ends.
    let mut text: Vec<String> = Vec::with_capacity(r.lines.len());
    for l in r.lines {
        if l.is_empty() && text.last().map_or(true, |p: &String| p.is_empty()) {
            continue;
        }
        text.push(l);
    }
    while text.last().is_some_and(|l| l.is_empty()) {
        text.pop();
    }
    page.text = text;
    page
}

/// Markers of a template or a component: HTML that a program fills in, which a person edits as
/// source. A file that holds one is never read as a page.
pub fn template_syntax(text: &str) -> bool {
    const MARKS: [&str; 17] = [
        "{{",
        "{%",
        "{#",
        "<%",
        "<?php",
        "<?=",
        " v-if=",
        " v-for=",
        " v-bind",
        " v-model",
        " ng-if",
        " ng-repeat",
        "*ngIf",
        "*ngFor",
        " x-data=",
        " @click=",
        "<ng-",
    ];
    MARKS.iter().any(|m| text.contains(m))
}

/// An MCP tool that reads a file from the disk: its result is the file as it is, and for an HTML
/// file that may be source being edited.
pub fn reads_local_files(tool: &str) -> bool {
    let t = tool.to_ascii_lowercase();
    [
        "read_file",
        "read_text_file",
        "read_multiple",
        "get_file",
        "filesystem",
        "__fs__",
        "read_resource",
    ]
    .iter()
    .any(|m| t.contains(m))
}

/// Does the path name an HTML file?
pub fn is_html_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".html") || p.ends_with(".htm") || p.ends_with(".xhtml")
}

/// The view of a page: what it is, then its text, up to `budget` tokens. `id` names the stored
/// raw HTML for `dtk html`.
pub fn summary(page: &Page, raw: &str, id: &str, status: Option<&str>, budget: usize) -> String {
    summary_with(page, raw, id, status, budget, None)
}

/// The same, with a line of explanation under the first line (for a file that was read).
pub fn summary_with(
    page: &Page,
    raw: &str,
    id: &str,
    status: Option<&str>,
    budget: usize,
    note: Option<&str>,
) -> String {
    let text_tokens: usize = page.text.iter().map(|l| l.len() / 4 + 1).sum();
    let c = &page.counts;
    let external = page
        .links
        .iter()
        .filter(|(_, h)| h.starts_with("http"))
        .count();
    let mut head = format!(
        "[dtk] web page{}: {} tokens of HTML, {} of text. The markup, scripts and styles are left out; the page is saved as {id}.\n",
        if page.title.is_empty() { String::new() } else { format!(" \"{}\"", page.title) },
        est_tokens(raw),
        text_tokens
    );
    if let Some(n) = note {
        head.push_str(&format!("{n}\n"));
    }
    if let Some(s) = status {
        head.push_str(&format!("response: {s}\n"));
    }
    if !page.description.is_empty() {
        head.push_str(&format!(
            "description: {}\n",
            crate::lean::trunc(&page.description, 240)
        ));
    }
    let mut parts = vec![format!(
        "{} links ({} external)",
        page.links.len(),
        external
    )];
    if !page.tables.is_empty() {
        parts.push(format!("{} tables", page.tables.len()));
    }
    if c.images > 0 {
        parts.push(format!("{} images", c.images));
    }
    if c.forms > 0 {
        parts.push(format!("{} forms", c.forms));
    }
    if c.scripts > 0 {
        parts.push(format!(
            "{} scripts ({}, left out)",
            c.scripts,
            crate::lean::human_bytes(c.script_bytes)
        ));
    }
    if !page.data.is_empty() {
        let kinds: Vec<String> = page
            .data
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, b)| format!("#{i} {} {}", b.kind, crate::lean::human_bytes(b.bytes)))
            .collect();
        parts.push(format!("embedded JSON: {}", kinds.join(", ")));
    }
    head.push_str(&format!("{}\n", parts.join(", ")));

    let room = budget.saturating_sub(est_tokens(&head) + 120);
    let mut body = String::new();
    let mut shown = 0;
    let mut used = 0;
    for l in &page.text {
        let cost = l.len() / 4 + 1;
        if used + cost > room && shown > 0 {
            break;
        }
        body.push_str(&crate::lean::trunc(l, 700));
        body.push('\n');
        used += cost;
        shown += 1;
    }
    let mut out = head;
    if shown < page.text.len() {
        // The text is longer than the budget: say what the page holds, then show its start.
        if page.headings.len() > 1 {
            out.push_str("headings:\n");
            for (level, h, at) in page.headings.iter().take(24) {
                out.push_str(&format!(
                    "  {}{} (line {})\n",
                    "  ".repeat(level.saturating_sub(1).min(3)),
                    crate::lean::trunc(h, 90),
                    at + 1
                ));
            }
            if page.headings.len() > 24 {
                out.push_str(&format!("  ... {} more\n", page.headings.len() - 24));
            }
        }
        out.push_str("text:\n");
        out.push_str(&body);
        out.push_str(&format!(
            "... {} more lines. dtk html {id} text {shown}   |   dtk html {id} grep WORD\n",
            page.text.len() - shown
        ));
    } else {
        out.push_str(&body);
    }
    out.push_str(&format!(
        "more: dtk html {id} headings|links|tables [N]|json [N]|grep WORD|text FROM [COUNT]"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><title>Orders &amp; returns</title>
<meta name="description" content="How orders work">
<style>body { color: red }</style>
<script>window.x = {"a": 1}; if (1 < 2) { document.write("<b>nope</b>") }</script>
<script type="application/ld+json">{"@type": "Product", "name": "Widget"}</script>
</head><body>
<nav><a href="/">Home</a> <a href="/shop">Shop</a> <a href="/about">About</a></nav>
<h1>Orders &amp; returns</h1>
<p>You can return an order within <b>30 days</b>. See the <a href="/policy?x=1&amp;y=2">full policy</a>
and the <a href="#top">top</a>.</p>
<!-- a comment that must not show -->
<h2>Steps</h2>
<ol><li>Pack it</li><li>Print the label<ul><li>on A4</li></ul></li></ol>
<table><tr><th>Region</th><th>Days</th></tr><tr><td>US</td><td>30</td></tr><tr><td>EU</td><td>14</td></tr></table>
<pre>$ return --order 42
done</pre>
<p style="display:none">hidden text</p>
<img src="a.png" alt="A parcel"><img src="b.png">
<svg><text>vector text</text></svg>
<footer>&copy; 2026 Example</footer>
</body></html>"##;

    #[test]
    fn reads_a_page_the_way_a_person_does() {
        let p = parse(PAGE);
        assert_eq!(p.title, "Orders & returns");
        assert_eq!(p.description, "How orders work");
        let text = p.text.join("\n");
        assert!(text.contains("# Orders & returns"), "{text}");
        assert!(text.contains("You can return an order within 30 days. See the [full policy](/policy?x=1&y=2) and the top."), "{text}");
        assert!(text.contains("## Steps"), "{text}");
        assert!(
            text.contains("1. Pack it\n2. Print the label\n  - on A4"),
            "{text}"
        );
        assert!(
            text.contains("| Region | Days |\n| US | 30 |\n| EU | 14 |"),
            "{text}"
        );
        assert!(
            text.contains("```\n$ return --order 42\ndone\n```"),
            "{text}"
        );
        assert!(text.contains("![A parcel]"), "{text}");
        assert!(text.contains("© 2026 Example"), "{text}");
        assert!(text.contains("(navigation menu: 3 links)"), "{text}");
        for gone in [
            "color: red",
            "document.write",
            "nope",
            "comment",
            "hidden text",
            "vector text",
        ] {
            assert!(!text.contains(gone), "{gone} must be left out: {text}");
        }
        assert_eq!(
            p.links.len(),
            1,
            "only a link with an address and text, not `#top` or the menu: {:?}",
            p.links
        );
        assert_eq!(p.tables.len(), 1);
        assert_eq!(p.data.len(), 1);
        assert_eq!(p.data[0].kind, "application/ld+json");
        assert_eq!(p.counts.scripts, 2);
        assert_eq!(p.headings.len(), 2);
    }

    #[test]
    fn a_space_in_its_own_element_is_a_space() {
        let p = parse("<html><body><p><em><span>class</span><span> </span></em><span>csv.</span><span>DictReader</span>(f)</p></body></html>");
        assert_eq!(p.text, ["class csv.DictReader(f)"]);
    }

    #[test]
    fn a_button_does_not_run_into_the_heading_beside_it() {
        let p = parse("<html><body><h1>Crate <span>dtk</span><button>Copy item path</button></h1></body></html>");
        assert_eq!(p.text, ["# Crate dtk Copy item path"]);
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &mdash; &nbsp;x &bogus; &"),
            "a & b <c> AB —  x &bogus; &"
        );
    }

    #[test]
    fn a_page_is_recognised_after_http_headers_and_not_in_other_text() {
        assert!(looks_like_html(PAGE));
        assert!(looks_like_html(&format!(
            "HTTP/2 200\ncontent-type: text/html\n\n{PAGE}"
        )));
        assert!(!looks_like_html("<html>"), "no body");
        assert!(!looks_like_html(
            "Here is some HTML: <!doctype html><html><body>x</body></html>"
        ));
        assert!(!looks_like_html(
            "fn main() { println!(\"<html><body>\"); }"
        ));
    }

    #[test]
    fn survives_broken_markup() {
        let p = parse("<html><body><p>one<p>two <b>bold <i>both</b> italic</i><div>three</body>");
        assert_eq!(p.text, ["one", "", "two bold both italic", "three"]);
        let q = parse("<html><body>a < b and c > d <a href='x'>unclosed");
        assert!(q.text.join(" ").contains("a < b and c > d"), "{:?}", q.text);
    }

    #[test]
    fn the_summary_shows_the_text_and_says_what_it_left_out() {
        let p = parse(PAGE);
        let s = summary(&p, PAGE, "abc123def456", None, 2500);
        assert!(s.starts_with("[dtk] web page \"Orders & returns\":"), "{s}");
        assert!(s.contains("saved as abc123def456"), "{s}");
        assert!(s.contains("2 scripts"), "{s}");
        assert!(s.contains("embedded JSON: #0 application/ld+json"), "{s}");
        assert!(s.contains("1 tables"), "{s}");
        assert!(s.contains("dtk html abc123def456 headings|links"), "{s}");
        assert!(est_tokens(&s) < est_tokens(PAGE) + 80);
    }
}
