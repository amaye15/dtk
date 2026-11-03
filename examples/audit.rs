//! Audit what the readers make of real output. For each file given, one line:
//!
//! `format  rows x columns  non-blank lines  lines the table does not account for  headers`
//!
//! Lines "not accounted for" are neither rows, header, border nor kept notes: a reader that drops
//! them without a word has taken a document or a list for a table. The numbers to watch are 0 in
//! the fourth column and sensible names in the fifth. Collect real output into files first, for
//! example the big results in `~/.claude/projects/*/*.jsonl`, then:
//!
//! `cargo run --release --example audit -- /path/to/outputs/*.txt`
use dtk::table::{detect, detect_wrapped, Format};

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (t, from, to) = match detect(&text) {
            Some(t) => (t, 0, text.len()),
            None => match detect_wrapped(&text) {
                Some(w) => (w.table, w.start, w.end),
                None => continue,
            },
        };
        let body = &text[from..to];
        let nonblank = body.lines().filter(|l| !l.trim().is_empty()).count();
        let one_line_per_row = !matches!(
            t.format,
            Format::JsonArray
                | Format::JsonObject
                | Format::Html
                | Format::PyRepr
                | Format::Csv
                | Format::Tsv
                | Format::Records
        );
        let borders = body
            .lines()
            .filter(|l| {
                let s = l.trim();
                !s.is_empty()
                    && s.chars()
                        .all(|c| "+-=|─│┌┐└┘├┤┬┴┼═╪╞╡╤╧╔╗╚╝╠╣╦╩╬ :".contains(c))
            })
            .count();
        let left = if one_line_per_row {
            // The note that says "N other lines" is not a line of the text.
            let notes = t
                .notes
                .iter()
                .filter(|n| !n.contains("other lines are not rows"))
                .count();
            nonblank
                .saturating_sub(t.rows.len() + 1 + notes + borders)
                .to_string()
        } else {
            "-".to_string()
        };
        let headers: Vec<&str> = t.headers.iter().take(8).map(String::as_str).collect();
        println!(
            "{}\t{}x{}\t{}\t{}\t{}\t{}",
            t.format.name(),
            t.rows.len(),
            t.headers.len(),
            nonblank,
            left,
            headers.join(" | "),
            path.rsplit('/').next().unwrap_or(&path)
        );
    }
}
