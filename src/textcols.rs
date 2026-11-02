//! Tables that are neither delimited nor boxed, and that `parse_aligned` cannot read because one
//! column holds free text with spaces in it:
//!
//! - `ps aux`, `ps -ef`, `df -h`: a header line, then rows whose last column
//!   (`COMMAND`, `Mounted on`) runs to the end of the line.
//!
//! `ls -l` and other listings are left alone on purpose: the names are the content, and a summary
//! would hide them.
//!
//! Reading these wrongly would put a value under the wrong column name, which is worse than not
//! reading them. So the checks are strict: every column before the last must hold one kind of
//! value, and at least two of them must be numbers. Prose and source code fail that.

use crate::table::{is_rule_line, valid_header, Format, Table, MIN_ROWS};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    Number,
    Time,
    Text,
}

/// `12`, `0.5`, `4%`, `1,234`, `460Gi`, `3.2G`, `12k`, `-7`.
fn is_number(tok: &str) -> bool {
    let t = tok.trim_end_matches('%').replace(',', "");
    let t = t.trim_start_matches(['+', '-']);
    let digits_end = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(digits_end);
    if num.is_empty() || num.parse::<f64>().is_err() {
        return false;
    }
    matches!(
        unit.to_ascii_lowercase().as_str(),
        "" | "k"
            | "m"
            | "g"
            | "t"
            | "p"
            | "b"
            | "kb"
            | "mb"
            | "gb"
            | "tb"
            | "ki"
            | "mi"
            | "gi"
            | "ti"
            | "kib"
            | "mib"
            | "gib"
            | "tib"
    )
}

/// A time or a date: `9:01AM`, `0:00.12`, `10:45:01`, `2026-11-02`, `19Oct26`, `Tue08PM`. A
/// column such as `ps`'s STARTED shows the time for today and the date for older rows.
fn is_time(tok: &str) -> bool {
    if !tok.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let plain = |c: char| c.is_ascii_digit() || matches!(c, ':' | '.' | '-' | 'T' | '+' | 'Z');
    let t = tok.trim_end_matches(|c: char| c.is_ascii_alphabetic() && !plain(c));
    if (tok.contains(':') || t.matches('-').count() == 2) && t.chars().all(plain) && t.len() >= 4 {
        return true;
    }
    // `19Oct26`, `Oct19`, `Tue08PM`: letters (a month or a weekday) and digits, one run each.
    let bytes = tok.as_bytes();
    let letters = bytes.iter().filter(|b| b.is_ascii_alphabetic()).count();
    let digits = bytes.iter().filter(|b| b.is_ascii_digit()).count();
    let runs = tok
        .chars()
        .fold((0, None), |(n, prev): (i32, Option<bool>), c| {
            let d = c.is_ascii_digit();
            (if prev == Some(d) { n } else { n + 1 }, Some(d))
        })
        .0;
    tok.chars().all(|c| c.is_ascii_alphanumeric())
        && (3..=5).contains(&letters)
        && (2..=4).contains(&digits)
        && runs <= 3
        && [
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            "mon", "tue", "wed", "thu", "fri", "sat", "sun",
        ]
        .iter()
        .any(|m| tok.to_ascii_lowercase().contains(m))
}

fn shape(tok: &str) -> Shape {
    if is_number(tok) {
        Shape::Number
    } else if is_time(tok) {
        Shape::Time
    } else {
        Shape::Text
    }
}

/// The first `n - 1` whitespace-separated tokens, then the rest of the line as the last cell.
fn split_capped(line: &str, n: usize) -> Option<Vec<String>> {
    let mut rest = line.trim_start();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n - 1 {
        let end = rest.find(char::is_whitespace)?;
        out.push(rest[..end].to_string());
        rest = rest[end..].trim_start();
    }
    let last = rest.trim_end();
    if last.is_empty() {
        return None;
    }
    out.push(last.to_string());
    Some(out)
}

/// What the header line can be called: its words, or its words with the last two joined
/// (`Mounted on`). A last word like "on" is a sign of the second reading, so that one goes first.
fn header_candidates(line: &str) -> Vec<Vec<String>> {
    let words: Vec<String> = line.split_whitespace().map(String::from).collect();
    if words.len() < 4 {
        return vec![words];
    }
    let mut merged = words[..words.len() - 2].to_vec();
    merged.push(format!(
        "{} {}",
        words[words.len() - 2],
        words[words.len() - 1]
    ));
    let last = words[words.len() - 1].as_str();
    if ["on", "to", "of", "in", "by", "at", "for"].contains(&last) {
        vec![merged, words]
    } else {
        vec![words, merged]
    }
}

/// A table with a header line and a free-text last column.
pub fn parse_tail_text(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !is_rule_line(l))
        .collect();
    if lines.len() < MIN_ROWS + 1 {
        return None;
    }
    // The header is the first line, or the second or third when a title line comes first
    // (a title above the header).
    for start in 0..lines.len().min(3) {
        for headers in header_candidates(lines[start]) {
            if let Some(t) = try_headers(&lines, start, headers) {
                return Some(t);
            }
        }
    }
    None
}

fn try_headers(lines: &[&str], start: usize, headers: Vec<String>) -> Option<Table> {
    let n = headers.len();
    if n < 3 || !valid_header(&headers) {
        return None;
    }
    let body = &lines[start + 1..];
    let mut rows = Vec::new();
    let mut notes: Vec<String> = lines[..start]
        .iter()
        .map(|l| l.trim().to_string())
        .collect();
    let mut bad = 0usize;
    for l in body {
        match split_capped(l, n) {
            Some(cells) if cells[..n - 1].iter().all(|c| c.chars().count() <= 40) => {
                rows.push(cells)
            }
            _ => {
                bad += 1;
                if notes.len() < 5 {
                    notes.push(l.trim().to_string());
                }
            }
        }
    }
    if rows.len() < MIN_ROWS || bad * 20 > rows.len() {
        return None;
    }
    // Free text varies in length. A last cell that always has the same number of words (two or
    // more) is really that many columns, and the header is missing their names: pandas prints an
    // unnamed index and splits a datetime that way. The labels would be wrong, so refuse.
    let words: Vec<usize> = rows
        .iter()
        .map(|r| r[n - 1].split_whitespace().count())
        .collect();
    if let Some((k, same)) = crate::table::mode(&words) {
        if k >= 2 && same * 100 >= words.len() * 95 {
            return None;
        }
    }
    // Every column but the last holds one kind of value, and two or more hold numbers.
    let mut number_cols = 0;
    for c in 0..n - 1 {
        let shapes: Vec<Shape> = rows.iter().map(|r| shape(&r[c])).collect();
        let count = |s: Shape| shapes.iter().filter(|x| **x == s).count();
        let top = [Shape::Number, Shape::Time, Shape::Text]
            .into_iter()
            .max_by_key(|s| count(*s))?;
        if count(top) * 100 < shapes.len() * 85 {
            return None;
        }
        if top == Shape::Number {
            number_cols += 1;
        }
    }
    if number_cols < 2 {
        return None;
    }
    Some(Table {
        format: Format::TailText,
        headers,
        rows,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commands differ in length, as real ones do.
    const ARGS: [&str; 4] = [
        "",
        " --type=renderer",
        " --type=gpu --id=7 --verbose",
        " -x",
    ];

    fn ps_aux(n: usize) -> String {
        let mut s = String::from(
            "USER               PID  %CPU %MEM      VSZ    RSS   TT  STAT STARTED      TIME COMMAND\n",
        );
        for i in 0..n {
            s.push_str(&format!(
                "{:<14} {:>7} {:>5.1} {:>4.1} {:>8} {:>6}   ??  Ss    9:{:02}AM   0:{:02}.{:02} /Applications/App {i}.app/Contents/MacOS/helper{}\n",
                ["root", "andrew", "_windowserver"][i % 3],
                100 + i * 7,
                (i % 9) as f64 * 1.3,
                (i % 5) as f64 * 0.4,
                400_000 + i * 1000,
                20_000 + i * 50,
                i % 60,
                i % 60,
                i % 100,
                ARGS[i % 4]
            ));
        }
        s
    }

    #[test]
    fn reads_ps_aux_with_the_command_in_one_cell() {
        let t = parse_tail_text(&ps_aux(40)).expect("ps aux");
        assert_eq!(t.format, Format::TailText);
        assert_eq!(t.headers.len(), 11);
        assert_eq!(t.headers[0], "USER");
        assert_eq!(t.headers[10], "COMMAND");
        assert_eq!(t.rows.len(), 40);
        assert!(t.rows[3][10].starts_with("/Applications/App 3.app/"));
        assert!(t.rows[2][10].ends_with("--id=7 --verbose"));
        assert_eq!(t.rows[0][1], "100");
    }

    #[test]
    fn a_started_column_may_mix_times_and_dates() {
        let mut s = String::from("USER PID %CPU %MEM VSZ RSS TT STAT STARTED TIME COMMAND\n");
        for i in 0..30 {
            let started = if i % 3 == 0 {
                "19Oct26".to_string()
            } else {
                format!("{}:{:02}PM", 1 + i % 11, i)
            };
            s.push_str(&format!(
                "root {} 0.{} 0.1 {} {} ?? Ss {} 0:00.{:02} /usr/sbin/daemon{}\n",
                100 + i,
                i % 10,
                400_000 + i,
                9_000 + i,
                started,
                i,
                ARGS[i % 4]
            ));
        }
        let t = parse_tail_text(&s).expect("ps with mixed STARTED");
        assert_eq!(t.headers[8], "STARTED");
        assert_eq!(t.rows.len(), 30);
    }

    #[test]
    fn reads_df_with_a_two_word_last_header() {
        let mut s = String::from(
            "Filesystem        Size    Used   Avail Capacity iused ifree %iused  Mounted on\n",
        );
        for i in 0..14 {
            // Most mount points are one word; a few have a space in them.
            let mount = if i % 5 == 0 {
                format!("/Volumes/Disk {i}")
            } else {
                format!("/Volumes/D{i}")
            };
            s.push_str(&format!(
                "/dev/disk{i}s1   460Gi   {}Gi   301Gi     {}%   {}k  3.2G     0%   {mount}\n",
                i + 10,
                i + 3,
                400 + i
            ));
        }
        let t = parse_tail_text(&s).expect("df");
        assert_eq!(t.headers.last().unwrap(), "Mounted on");
        assert_eq!(t.headers.len(), 9);
        assert_eq!(t.rows[5][8], "/Volumes/Disk 5");
        assert_eq!(t.rows[2][8], "/Volumes/D2");
    }

    #[test]
    fn a_last_cell_of_constant_word_count_is_columns_without_names() {
        // pandas with an unnamed index and a datetime split in two: the header has 6 names, the
        // rows have 8 values. Taking the last three for one cell would put values under wrong names.
        let mut s = String::from(
            "order_id                  ts   customer      region  amount  latency_ms\n",
        );
        for i in 0..30 {
            s.push_str(&format!(
                "{i}     {} 2026-10-01 00:{:02}:00  cust_{i:04}  us-east  {}.5  {}\n",
                100000 + i,
                i,
                i * 3,
                100 + i
            ));
        }
        assert!(parse_tail_text(&s).is_none());
    }

    #[test]
    fn prose_is_not_a_table() {
        let prose =
            "The quick brown fox jumps over the lazy dog and keeps running far away\n".repeat(30);
        assert!(parse_tail_text(&prose).is_none());
        let story = (0..30)
            .map(|i| format!("Chapter {i} begins with a long walk through the old town square"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_tail_text(&story).is_none());
    }

    #[test]
    fn source_code_and_build_output_are_not_tables() {
        let code =
            "fn main() {\n    let a = f(1, 2, 3);\n    println!(\"{}\", g(a, b));\n}\n".repeat(20);
        assert!(parse_tail_text(&code).is_none());
        let build = (0..30)
            .map(|i| format!("   Compiling crate{i} v0.{i}.0 (/home/user/src/crate{i})"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_tail_text(&build).is_none());
    }

    #[test]
    fn a_column_that_mixes_kinds_is_a_misalignment_not_a_table() {
        // The third column is a number in some rows and a word in others: rows are shifted.
        let mut s = String::from("NAME PID SIZE NODE LABEL\n");
        for i in 0..30 {
            let size = if i % 2 == 0 { "100" } else { "TCP" };
            s.push_str(&format!("proc{i} {i} {size} 7 label{}\n", ARGS[i % 4]));
        }
        assert!(parse_tail_text(&s).is_none());
    }
}
