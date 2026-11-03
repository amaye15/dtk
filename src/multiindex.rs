//! pandas output with a MultiIndex: `df.groupby([a, b]).agg(...)` and `Series.value_counts()`.
//!
//! pandas leaves a repeated index label blank (`ap  sku_000 ...` then `    sku_001 ...`), so
//! the rows do not all have the same number of cells and a split on spaces gives wrong columns.
//! This parser reads by character position instead. The line of index names
//! (`region product`) marks where the index levels start. Values are right-aligned, so the
//! right edges of one full row mark where the value columns end. A header label belongs to the
//! column that ends where the label ends, and a blank top-level label repeats the one to its left.
//!
//! A line that does not fit is refused, not guessed at: a table with a column in the wrong
//! place is worse than no table.

use crate::table::{valid_header, Format, Table, MIN_ROWS};
use std::collections::HashMap;

/// Most header lines above the line of index names (`agg` with two levels is the usual case).
const MAX_HEADER_LINES: usize = 3;

struct Tok {
    start: usize,
    end: usize,
    text: String,
}

/// Whitespace-separated words with their character positions (`end` is one past the last).
fn tokens(line: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut cur: Option<(usize, String)> = None;
    let mut n = 0;
    for (i, ch) in line.chars().enumerate() {
        n = i + 1;
        if ch.is_whitespace() {
            if let Some((start, text)) = cur.take() {
                out.push(Tok {
                    start,
                    end: i,
                    text,
                });
            }
        } else {
            cur.get_or_insert_with(|| (i, String::new())).1.push(ch);
        }
    }
    if let Some((start, text)) = cur {
        out.push(Tok {
            start,
            end: n,
            text,
        });
    }
    out
}

/// The line under a Series or a cut-down frame.
enum Footer {
    /// `Name: count, dtype: int64`, `Length: 80, dtype: float64` or `dtype: int64`; the name when
    /// there is one.
    Series(Option<String>),
    /// `[240 rows x 7 columns]`
    Frame,
}

fn footer(line: &str) -> Option<Footer> {
    let t = line.trim();
    if t.starts_with('[') && t.ends_with("columns]") && t.contains(" rows x ") {
        return Some(Footer::Frame);
    }
    if !t.contains("dtype:") {
        return None;
    }
    let name = t
        .strip_prefix("Name: ")
        .and_then(|r| r.split(", ").next())
        .map(str::to_string);
    (name.is_some() || t.starts_with("Length: ") || t.starts_with("dtype: "))
        .then_some(Footer::Series(name))
}

fn looks_like_name(t: &str) -> bool {
    t.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && t.parse::<f64>().is_err()
}

/// pandas output with a MultiIndex, or a Series with a name line and a `dtype:` footer.
pub fn parse_multiindex(text: &str) -> Option<Table> {
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < MIN_ROWS + 1 {
        return None;
    }
    let foot = lines.last().and_then(|l| footer(l));
    if foot.is_some() {
        lines.pop();
    }
    match foot {
        // A Series: the line of index names comes first.
        Some(Footer::Series(name)) => try_layout(&lines, 0, name.as_deref()),
        // A frame: one to three header lines, then the line of index names.
        _ => (1..=MAX_HEADER_LINES).find_map(|n| try_layout(&lines, n, None)),
    }
}

fn try_layout(lines: &[&str], header_lines: usize, series_name: Option<&str>) -> Option<Table> {
    let series = header_lines == 0;
    let names = tokens(lines.get(header_lines)?);
    let k = names.len();
    if k == 0 || !names.iter().all(|t| looks_like_name(&t.text)) {
        return None;
    }
    let level_starts: Vec<usize> = names.iter().map(|t| t.start).collect();
    let body = &lines[header_lines + 1..];
    if body.len() < MIN_ROWS {
        return None;
    }

    // Each body line is cut into its index cells (by position) and its value words.
    let mut parsed: Vec<(usize, Option<Split>)> = Vec::new();
    let mut counts: HashMap<usize, usize> = HashMap::new();
    let mut failed = 0usize;
    for (i, line) in body.iter().enumerate() {
        let split = split_row(line, &level_starts);
        if let Some(sp) = &split {
            *counts.entry(sp.values.len()).or_default() += 1;
        }
        if split.is_none() {
            failed += 1;
        }
        parsed.push((i, split));
        // Give up early on text that is not this layout at all.
        if (i == 12 && counts.is_empty()) || (i >= 12 && failed * 5 > i + 1) {
            return None;
        }
    }
    let (v, _) = counts.into_iter().max_by_key(|(n, c)| (*c, *n))?;
    if v == 0 || (series && v != 1) {
        return None;
    }
    // Every value column ends at the same place in every row. One full row says where.
    let reference = parsed
        .iter()
        .filter_map(|(_, sp)| sp.as_ref())
        .find(|sp| sp.values.len() == v)?;
    let value_ends: Vec<usize> = reference.values.iter().map(|t| t.end).collect();

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut bad = 0usize;
    for (i, split) in &parsed {
        let row = split.as_ref().and_then(|sp| {
            let ends_match =
                sp.values.len() == v && sp.values.iter().zip(&value_ends).all(|(t, e)| t.end == *e);
            if !ends_match {
                return None;
            }
            let mut cells: Vec<String> = Vec::with_capacity(k + v);
            if sp.first > 0 {
                cells.extend_from_slice(&rows.last()?[..sp.first]);
            }
            cells.extend(sp.index.iter().cloned());
            cells.extend(sp.values.iter().map(|t| t.text.clone()));
            Some(cells)
        });
        match row {
            Some(r) => rows.push(r),
            None => {
                bad += 1;
                if notes.len() < 5 {
                    notes.push(body[*i].trim().to_string());
                }
                if bad > 2 && bad * 10 > rows.len() + bad {
                    return None;
                }
            }
        }
    }
    if rows.len() < MIN_ROWS || bad * 20 > rows.len() {
        return None;
    }

    // Header labels, one list per level. Each goes to the value column that ends where it ends.
    let mut levels: Vec<Vec<Option<String>>> = Vec::new();
    let mut level_names: Vec<Option<String>> = Vec::new();
    for line in &lines[..header_lines] {
        let mut level = vec![None; v];
        let mut name = None;
        for t in tokens(line) {
            match value_ends.iter().position(|e| *e == t.end) {
                Some(col) => level[col] = Some(t.text),
                // The name of this level of columns (`year` in a pivot table) sits above the
                // index, at its left edge.
                None if t.start == level_starts[0] => name = Some(t.text),
                None => return None,
            }
        }
        levels.push(level);
        level_names.push(name);
    }
    let mut headers: Vec<String> = names.iter().map(|t| t.text.clone()).collect();
    if series {
        headers.push(series_name.unwrap_or("value").to_string());
    } else {
        // A blank top-level label repeats the one to its left (pandas "sparsifies" them). The
        // last level is never blank on its own, except where a column has no second label.
        let mut carry: Vec<Option<String>> = vec![None; levels.len()];
        for col in 0..v {
            let mut parts: Vec<String> = Vec::new();
            for (l, level) in levels.iter().enumerate() {
                let last = l + 1 == levels.len();
                if let Some(label) = &level[col] {
                    carry[l] = Some(label.clone());
                    parts.push(label.clone());
                } else if !last {
                    if let Some(c) = &carry[l] {
                        parts.push(c.clone());
                    }
                }
            }
            if parts.is_empty() {
                return None;
            }
            let mut name = parts.join(".");
            // A column labelled `2022` has no name of its own; the level's name tells what it is.
            if name.parse::<f64>().is_ok() {
                if let Some(Some(level)) = level_names.last() {
                    name = format!("{level}={name}");
                }
            }
            headers.push(name);
        }
    }
    if !valid_header(&headers) {
        return None;
    }
    Some(Table {
        format: Format::Aligned,
        headers,
        rows,
        notes,
    })
}

/// A body line, cut by position.
struct Split {
    /// The first index level this line shows; the levels before it are blank (same as above).
    first: usize,
    /// The index cells from `first` on.
    index: Vec<String>,
    /// The words after the index, with their positions.
    values: Vec<Tok>,
}

/// Index cells are left-aligned under the index names, so they are cut at the position of the
/// next name. That keeps a label with a space in it (`New York`) whole. The last index cell ends
/// at the first run of two spaces; the words after it are the values.
fn split_row(line: &str, level_starts: &[usize]) -> Option<Split> {
    let chars: Vec<char> = line.chars().collect();
    let k = level_starts.len();
    let start = chars.iter().position(|c| !c.is_whitespace())?;
    let first = level_starts.iter().position(|s| *s == start)?;
    let text = |a: usize, b: usize| -> String {
        chars[a.min(chars.len())..b.min(chars.len())]
            .iter()
            .collect::<String>()
            .trim()
            .to_string()
    };
    let mut index = Vec::with_capacity(k - first);
    for l in first..k - 1 {
        let (a, b) = (level_starts[l], level_starts[l + 1]);
        // A label that runs into the next column is not where this layout puts it.
        if chars.get(b - 1).is_some_and(|c| !c.is_whitespace()) {
            return None;
        }
        index.push(text(a, b));
    }
    let a = level_starts[k - 1];
    let mut e = a;
    while e < chars.len()
        && !(chars[e].is_whitespace() && chars.get(e + 1).is_some_and(|c| c.is_whitespace()))
    {
        e += 1;
    }
    index.push(text(a, e));
    if index.iter().any(String::is_empty) {
        return None;
    }
    let mut values = tokens(&chars[e.min(chars.len())..].iter().collect::<String>());
    for t in &mut values {
        t.start += e;
        t.end += e;
    }
    Some(Split {
        first,
        index,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grouped(groups: usize, per: usize) -> String {
        let mut s = String::from(
            "                amount_sum  qty   n\nregion product                     \n",
        );
        for g in 0..groups {
            for p in 0..per {
                let region = if p == 0 {
                    format!("{:<6}", format!("r{g}"))
                } else {
                    " ".repeat(6)
                };
                s.push_str(&format!(
                    "{region} sku_{p:03}  {:>10.2}  {:>3} {:>3}\n",
                    1000.0 + (g * per + p) as f64 * 1.5,
                    10 + p,
                    3 + p % 7
                ));
            }
        }
        s
    }

    #[test]
    fn a_grouped_frame_keeps_every_row_and_every_label() {
        let t = parse_multiindex(&grouped(4, 20)).unwrap();
        assert_eq!(t.headers, ["region", "product", "amount_sum", "qty", "n"]);
        assert_eq!(t.rows.len(), 80);
        assert_eq!(t.rows[0][..2], ["r0", "sku_000"]);
        // A blank label repeats the one above.
        assert_eq!(t.rows[5][..2], ["r0", "sku_005"]);
        assert_eq!(t.rows[20][..2], ["r1", "sku_000"]);
        assert_eq!(t.rows[79][0], "r3");
    }

    const TWO_LEVELS: &str =
        include_str!("../tests/corpus/positive/pandas_groupby_two_level_header.txt");
    const SPACES: &str =
        include_str!("../tests/corpus/positive/pandas_groupby_labels_with_spaces.txt");
    const PIVOT: &str = include_str!("../tests/corpus/positive/pandas_pivot_multiindex.txt");

    #[test]
    fn two_header_levels_are_joined() {
        let t = parse_multiindex(TWO_LEVELS).unwrap();
        assert_eq!(
            t.headers,
            [
                "region",
                "product",
                "amount.sum",
                "amount.mean",
                "amount.max",
                "qty.sum",
                "qty.count"
            ]
        );
        assert_eq!(t.rows.len(), 160);
        // The first row of a group shows its region, the rest leave it blank in the text.
        assert!(t.rows.iter().all(|r| !r[0].is_empty() && !r[1].is_empty()));
        assert_eq!(t.rows[0][..2], ["ap", "sku_000"]);
        assert_eq!(t.rows[1][..2], ["ap", "sku_001"]);
        assert_eq!(t.rows[159][0], "us-west");
    }

    #[test]
    fn a_label_with_a_space_stays_whole() {
        let t = parse_multiindex(SPACES).unwrap();
        assert_eq!(t.rows.len(), 25);
        let cities: std::collections::BTreeSet<_> = t.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(
            cities.into_iter().collect::<Vec<_>>(),
            [
                "Austin",
                "Los Angeles",
                "New York",
                "San Francisco",
                "St. Louis"
            ]
        );
        assert!(t.rows.iter().any(|r| r[1] == "Home & Garden"));
    }

    #[test]
    fn the_name_of_a_column_level_is_not_a_column() {
        // `year` names the level of columns that holds 2023, 2024 and 2025.
        let t = parse_multiindex(PIVOT).unwrap();
        assert_eq!(t.headers.len(), 8);
        assert_eq!(t.headers[2], "amount.2023");
        assert_eq!(t.headers[7], "qty.2025");
        assert!(!t.headers.iter().any(|h| h == "year"));
    }

    #[test]
    fn a_series_needs_its_dtype_footer() {
        let mut s = String::from("product\n");
        for p in 0..40 {
            s.push_str(&format!("sku_{p:03}    {:>3}\n", 90 - p));
        }
        assert!(parse_multiindex(&s).is_none(), "no footer, no Series");
        s.push_str("Name: count, dtype: int64\n");
        let t = parse_multiindex(&s).unwrap();
        assert_eq!(t.headers, ["product", "count"]);
        assert_eq!(t.rows.len(), 40);
        assert_eq!(t.rows[0], ["sku_000", "90"]);
    }

    #[test]
    fn a_misaligned_row_is_refused_not_guessed() {
        let mut s = grouped(2, 20);
        s.push_str("garbage line that is not a row at all\n".repeat(8).as_str());
        assert!(parse_multiindex(&s).is_none());
    }

    #[test]
    fn plain_text_is_not_a_table() {
        let s = "Name Value\n".to_string() + &"alpha  1\n".repeat(30);
        assert!(parse_multiindex(&s).is_none());
    }
}
