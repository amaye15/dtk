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
    let null = |s: &str| matches!(s.trim(), "" | "NaN" | "nan" | "None" | "NULL");
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

fn same_row(got: &[String], want: &[String]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| same_cell(g, w))
}

/// DuckDB prints the type of each column in a row under the header, and shows only some of the
/// rows of a long result, with three rows of dots between them and a line that says how many rows
/// there are. The type row and the dots are not data. The CSV is DuckDB's own (`COPY ... (header)`).
#[test]
fn a_duckdb_table_is_read_without_its_type_row_and_its_dots() {
    let want = expected("duckdb_orders");
    let all = detect(&fixture("duckdb_box_all.txt")).expect("the whole result is read");
    assert_eq!(all.headers, want[0]);
    assert_eq!(all.rows.len(), 300, "the type row is not a row");
    for (r, (got, w)) in all.rows.iter().zip(&want[1..]).enumerate() {
        assert!(same_row(got, w), "row {r}: {got:?} and {w:?}");
    }
    assert!(
        all.notes
            .iter()
            .any(|n| n.starts_with("column types as printed: id int64, customer varchar")),
        "{:?}",
        all.notes
    );

    // `show(max_rows=20)`: ten rows, three rows of dots, ten rows.
    let part = detect(&fixture("duckdb_box_elided.txt")).expect("the part is read");
    assert_eq!(part.rows.len(), 20, "the dots are not rows");
    let expected_rows: Vec<&Vec<String>> = want[1..11].iter().chain(&want[291..301]).collect();
    for (r, (got, w)) in part.rows.iter().zip(expected_rows).enumerate() {
        assert!(same_row(got, w), "row {r}: {got:?} and {w:?}");
    }
    assert!(
        part.notes.iter().any(|n| n.contains("300 rows (20 shown)")),
        "{:?}",
        part.notes
    );
    assert!(
        part.notes.iter().any(|n| n.contains("elided rows")),
        "{:?}",
        part.notes
    );
    // The numbers of the first column are numbers: a dot in it would make it text.
    let ids: Vec<&str> = part.rows.iter().map(|r| r[0].as_str()).collect();
    assert!(ids.iter().all(|i| i.parse::<u32>().is_ok()), "{ids:?}");
}

/// Real PySpark output (Spark 4, pandas 3). `show()` is a boxed table. `toPandas().to_string()`
/// came with eleven lines of warnings in front of it (the command had `2>&1`), and Spark's stage
/// bar, drawn with `\r` on the very line where the table's header starts. The CSV is the data that
/// made both.
#[test]
fn real_spark_output_is_read_with_its_warnings_and_its_progress_bar() {
    let want = expected("spark_orders");
    let shown = detect(&fixture("spark_show.txt")).expect("show() is read");
    assert_eq!(shown.headers, want[0]);
    assert_eq!(shown.rows.len(), 300);
    for (r, (got, w)) in shown.rows.iter().zip(&want[1..]).enumerate() {
        assert!(same_row(got, w), "show() row {r}: {got:?} and {w:?}");
    }

    let text = fixture("spark_pandas_with_warnings.txt");
    assert!(text.contains('\r'), "the progress bar is in the fixture");
    // The fixture comes from a real run; dtk reads it through `detect_wrapped` as the hook does.
    let table = detect(&text)
        .or_else(|| dtk::table::detect_wrapped(&text).map(|w| w.table))
        .expect("the table after the warnings is read");
    assert_eq!(
        table.headers[1..],
        want[0][..],
        "the names of the columns are the table's"
    );
    assert_eq!(table.rows.len(), 300);
    for (r, (got, w)) in table.rows.iter().zip(&want[1..]).enumerate() {
        assert!(same_row(&got[1..], w), "row {r}: {got:?} and {w:?}");
    }
}

/// Lines of text in front of a table must never become its header. Before 0.20.0 the second
/// line of Spark's warnings was taken for the header of 300 rows, and the columns were called
/// `Using`, `Spark's`, `default`.
#[test]
fn words_in_front_of_a_table_are_never_its_column_names() {
    let mut text = String::from(
        "Using Spark's default log4j profile: org/apache/spark/log4j2-defaults.properties\n\
         Setting default log level to WARN and nothing else\n\
         To adjust logging level use sc.setLogLevel(newLevel) for everything here\n",
    );
    text.push_str("     id  customer     region  amount\n");
    for i in 0..60 {
        text.push_str(&format!(
            "{i:>3}   {i:>3}  cust_{i:03}  us-east  {}.50\n",
            i * 3
        ));
    }
    for table in [
        detect(&text),
        dtk::table::detect_wrapped(&text).map(|w| w.table),
    ]
    .into_iter()
    .flatten()
    {
        assert!(
            !table
                .headers
                .iter()
                .any(|h| h == "Using" || h == "Spark's" || h == "default"),
            "{:?}",
            table.headers
        );
        assert!(
            table.headers.iter().any(|h| h == "customer"),
            "{:?}",
            table.headers
        );
    }
}

/// A progress bar that was drawn over leaves its last version, as in a terminal. A carriage return
/// inside a cell is data and stays.
#[test]
fn a_progress_bar_is_collapsed_and_a_return_in_a_cell_is_not() {
    let mut bar = String::from(
        "\r  0%|          | 0/100 [00:00<?, ?it/s]\r 50%|█████     | 50/100 [00:01<00:01, 49it/s]\r100%|██████████| 100/100 [00:02<00:00, 49it/s]\n",
    );
    bar.push_str("id,name,amount,region\n");
    for i in 0..60 {
        bar.push_str(&format!(
            "{i},user_{},{}.5,{}\n",
            i % 7,
            i * 3,
            ["us", "eu", "ap"][i % 3]
        ));
    }
    let table = detect(&bar).or_else(|| dtk::table::detect_wrapped(&bar).map(|w| w.table));
    assert_eq!(table.expect("the CSV after the bar is read").rows.len(), 60);

    let mut quoted = String::from("id,note,amount,region\n");
    for i in 0..60 {
        quoted.push_str(&format!(
            "{i},\"line one\rline two {i}\",{}.5,{}\n",
            i * 3,
            ["us", "eu", "ap"][i % 3]
        ));
    }
    let t = detect(&quoted).expect("a CSV with a return in a cell");
    assert_eq!(
        t.rows[7][1], "line one\rline two 7",
        "the cell is as it was written"
    );
}

/// `print(df.collect())` in Spark prints a list of `Row(id=1, customer='cust_001', ...)`. The
/// names of the columns are in the text, and the table has them.
#[test]
fn a_list_of_spark_rows_is_a_table_with_the_names_of_the_columns() {
    let want = expected("spark_orders");
    let table = detect(&fixture("spark_collect.txt")).expect("the Rows are read");
    assert_eq!(table.headers, want[0]);
    assert_eq!(table.rows.len(), 300);
    for (r, (got, w)) in table.rows.iter().zip(&want[1..]).enumerate() {
        assert!(same_row(got, w), "row {r}: {got:?} and {w:?}");
    }
}
