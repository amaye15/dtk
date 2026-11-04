//! Wide pandas frames printed in blocks of columns (`display.width`), and frames of numbers with
//! a text label in each row (`df.corr()`).
//!
//! The fixtures are real pandas output. Next to each is `NAME.expected.csv`, which pandas wrote
//! from the same frame with `to_csv()`. A frame is right only when dtk's reading of the text equals
//! that file, cell by cell, so the check does not use dtk's own code to say what the frame is.

use dtk::table::detect;

fn fixture(name: &str) -> String {
    // On Windows, git may have checked the file out with CRLF line endings.
    std::fs::read_to_string(format!(
        "{}/tests/corpus/frames/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
    .replace("\r\n", "\n")
}

fn expected(name: &str) -> Vec<Vec<String>> {
    let text = fixture(&format!("{name}.expected.csv"));
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(text.as_bytes());
    rdr.records()
        .map(|r| r.unwrap().iter().map(String::from).collect())
        .collect()
}

fn same_cell(printed: &str, written: &str) -> bool {
    let null = |s: &str| matches!(s.trim(), "" | "NaN" | "nan" | "None");
    if null(printed) || null(written) {
        return null(printed) && null(written);
    }
    match (printed.trim().parse::<f64>(), written.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => (a - b).abs() <= 1e-9 * b.abs().max(1.0),
        _ => printed.trim() == written.trim(),
    }
}

const FRAMES: [&str; 6] = [
    "wide_frame_wrapped",
    "three_blocks",
    "last_block_one_column",
    "corr_wrapped",
    "corr_one_block",
    "named_index_wrapped",
];

#[test]
fn every_wrapped_frame_is_read_as_pandas_wrote_it() {
    for name in FRAMES {
        let table =
            detect(&fixture(&format!("{name}.txt"))).unwrap_or_else(|| panic!("{name}: not read"));
        let want = expected(name);
        // pandas writes an empty cell for an index with no name; dtk calls it `idx`.
        let index = if want[0][0].is_empty() {
            "idx"
        } else {
            want[0][0].as_str()
        };
        assert_eq!(table.headers[0], index, "{name}: the name of the index");
        assert_eq!(table.headers[1..], want[0][1..], "{name}: the column names");
        assert_eq!(
            table.rows.len(),
            want.len() - 1,
            "{name}: the number of rows"
        );
        for (r, (got, w)) in table.rows.iter().zip(&want[1..]).enumerate() {
            assert_eq!(
                got.len(),
                w.len(),
                "{name}: row {r} has the wrong number of cells"
            );
            for (c, (g, x)) in got.iter().zip(w).enumerate() {
                assert!(
                    same_cell(g, x),
                    "{name}: row {r}, column {} ({}): dtk read `{g}`, pandas wrote `{x}`",
                    c,
                    want[0][c]
                );
            }
        }
        assert!(table.notes.is_empty(), "{name}: {:?}", table.notes);
    }
}

/// The frame is joined only when the blocks agree. Each change below makes them disagree, and a
/// wrong table is worse than none.
#[test]
fn blocks_that_do_not_agree_are_not_joined() {
    let text = fixture("three_blocks.txt");
    let blocks: Vec<&str> = text.trim_end().split("\n\n").collect();
    assert_eq!(blocks.len(), 3);
    let rebuild = |b: &[String]| b.join("\n\n") + "\n";
    let owned: Vec<String> = blocks.iter().map(|b| b.to_string()).collect();

    // The same text, rebuilt unchanged, is read.
    assert!(detect(&rebuild(&owned)).is_some());

    // A row label in the second block is not the label in the first.
    let mut changed = owned.clone();
    changed[1] = changed[1].replacen("\n5", "\n9", 1);
    assert!(detect(&rebuild(&changed)).is_none(), "a different label");

    // A row is missing from the last block.
    let mut short = owned.clone();
    let mut lines: Vec<&str> = short[2].lines().collect();
    lines.remove(7);
    short[2] = lines.join("\n");
    assert!(detect(&rebuild(&short)).is_none(), "a row less");

    // The header of the second block does not end with the backslash.
    let mut plain = owned.clone();
    plain[1] = plain[1].replacen(" \\\n", "\n", 1);
    assert!(detect(&rebuild(&plain)).is_none(), "no backslash");

    // The same column in two blocks is not one frame.
    let mut twice = owned.clone();
    let name = owned[2]
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    twice[1] = twice[1].replacen("f00", &name, 1);
    assert!(detect(&rebuild(&twice)).is_none(), "a column named twice");
}

/// pandas hid 23 of the 31 columns and printed `...` for them. dtk reads the 8 that are printed,
/// as pandas wrote them, and says where the others were.
#[test]
fn a_frame_with_columns_that_pandas_hid_is_read_and_says_so() {
    let name = "columns_hidden_by_pandas";
    let table = detect(&fixture(&format!("{name}.txt"))).expect("read");
    let want = expected(name);
    assert_eq!(table.headers[1..], want[0][1..], "the printed columns");
    assert_eq!(table.rows.len(), want.len() - 1);
    for (r, (got, w)) in table.rows.iter().zip(&want[1..]).enumerate() {
        for (c, (g, x)) in got.iter().zip(w).enumerate() {
            assert!(
                same_cell(g, x),
                "row {r}, column {}: `{g}` and `{x}`",
                want[0][c]
            );
        }
    }
    let note = &table.notes[0];
    assert!(
        note.contains("hid some columns in this output, between amount and f20"),
        "{note}"
    );
    // pandas's own footer is kept, and it says how many columns the frame has.
    assert!(
        table
            .notes
            .iter()
            .any(|n| n.contains("[70 rows x 31 columns]")),
        "{:?}",
        table.notes
    );
}

/// Three dots that are data, or that stand in only some rows, are not an elided column.
#[test]
fn dots_that_are_not_a_hidden_column_change_nothing() {
    let mut text = String::from("    id  name  ...  score\n");
    for i in 0..20 {
        let dots = if i == 7 { "xyz" } else { "..." };
        text.push_str(&format!("{i:<3} {i:>3}  n{i:<2}  {dots}  {i}.5\n"));
    }
    // A row without the dots at that place: the text is read as it is, with its dots, or not at all.
    if let Some(t) = detect(&text) {
        assert!(
            t.notes.iter().all(|n| !n.contains("hid some columns")),
            "{:?}",
            t.notes
        );
    }
}
