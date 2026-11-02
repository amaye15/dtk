//! Gate G8: the drill-down commands (`dtk rows | top | count | where | stats | get`).
//!
//! A summary points the model at these commands, so they must give the right rows. Each result is
//! checked against arithmetic done here with the `csv` crate and plain sorting, not with dtk's code.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-query-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_dtk"))
        .args(args)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).to_string(),
        String::from_utf8_lossy(&o.stderr).to_string(),
    )
}

/// A table of 300 rows with text, numbers, a formatted number and placeholders.
fn orders() -> Vec<Vec<String>> {
    (0..300)
        .map(|i| {
            vec![
                format!("o{i:04}"),
                ["us", "eu", "ap"][i % 3].to_string(),
                format!("{}.{:02}", (i * 37) % 500, i % 100),
                if i % 10 == 3 {
                    "N/A".to_string()
                } else {
                    ((i * 7) % 90).to_string()
                },
                format!("${},{:03}.00", 1 + i % 4, (i * 13) % 1000),
                ["ok", "failed", "retry"][(i / 3) % 3].to_string(),
            ]
        })
        .collect()
}

fn write_csv(path: &Path, rows: &[Vec<String>]) {
    let mut w = csv::Writer::from_path(path).unwrap();
    w.write_record(["id", "region", "amount", "age", "price", "status"])
        .unwrap();
    for r in rows {
        w.write_record(r).unwrap();
    }
    w.flush().unwrap();
}

fn parse(out: &str) -> Vec<Vec<String>> {
    // keep the CSV part: stop at the first note line ("... N more", "N of M rows match")
    let body: Vec<&str> = out
        .lines()
        .take_while(|l| !l.starts_with("... ") && !l.contains(" rows match"))
        .collect();
    let joined = body.join("\n");
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(joined.as_bytes());
    rdr.records()
        .map(|r| r.unwrap().iter().map(String::from).collect())
        .collect()
}

fn fixture(name: &str) -> (PathBuf, PathBuf, Vec<Vec<String>>) {
    let home = temp_home(name);
    let rows = orders();
    let f = home.join("orders.csv");
    write_csv(&f, &rows);
    (home, f, rows)
}

#[test]
fn rows_gives_the_rows_you_ask_for() {
    let (home, f, rows) = fixture("rows");
    let (code, out, _) = dtk(&home, &["rows", f.to_str().unwrap(), "100", "5"]);
    assert_eq!(code, 0);
    let got = parse(&out);
    assert_eq!(got[0][0], "id");
    assert_eq!(&got[1..], &rows[100..105]);
    assert!(out.contains("more rows after row 104"), "{out}");
    // past the end: only what exists
    let (_, out, _) = dtk(&home, &["rows", f.to_str().unwrap(), "298", "50"]);
    assert_eq!(&parse(&out)[1..], &rows[298..]);
}

#[test]
fn top_gives_the_largest_numbers_and_reads_formatted_ones() {
    let (home, f, rows) = fixture("top");
    let file = f.to_str().unwrap();
    // amount is plain, price is "$2,013.00"
    for (col, idx) in [("amount", 2usize), ("price", 4)] {
        let num = |s: &str| s.replace(['$', ','], "").parse::<f64>().ok();
        let mut sorted: Vec<(usize, f64)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| num(&r[idx]).map(|v| (i, v)))
            .collect();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let (_, out, _) = dtk(&home, &["top", file, col, "7"]);
        let got = parse(&out);
        let want: Vec<&Vec<String>> = sorted.iter().take(7).map(|(i, _)| &rows[*i]).collect();
        assert_eq!(got.len(), 8, "{col}: header and 7 rows");
        for (g, w) in got[1..].iter().zip(want) {
            assert_eq!(g, w, "{col}");
        }
        let (_, asc, _) = dtk(&home, &["top", file, col, "3", "--asc"]);
        let low = parse(&asc);
        let mut ascending = sorted.clone();
        ascending.reverse();
        // ties: the smallest value first, then the earliest row
        let min = ascending[0].1;
        assert_eq!(num(&low[1][idx]).unwrap(), min, "{col} --asc");
    }
    // placeholders are left out and said so
    let (_, out, _) = dtk(&home, &["top", file, "age", "3"]);
    assert!(out.contains("30 rows have no number in \"age\""), "{out}");
}

#[test]
fn count_matches_a_count_made_here() {
    let (home, f, rows) = fixture("count");
    let (_, out, _) = dtk(&home, &["count", f.to_str().unwrap(), "status"]);
    let got = parse(&out);
    assert_eq!(got[0], ["status", "count"]);
    for g in &got[1..] {
        let want = rows.iter().filter(|r| r[5] == g[0]).count();
        assert_eq!(g[1].parse::<usize>().unwrap(), want, "{}", g[0]);
    }
    assert_eq!(
        got[1..]
            .iter()
            .map(|g| g[1].parse::<usize>().unwrap())
            .sum::<usize>(),
        300
    );
}

#[test]
fn where_filters_with_every_operator() {
    let (home, f, rows) = fixture("where");
    let file = f.to_str().unwrap();
    type Case<'a> = (&'a str, &'a str, &'a str, Box<dyn Fn(&Vec<String>) -> bool>);
    let cases: Vec<Case> = vec![
        ("region", "=", "eu", Box::new(|r| r[1] == "eu")),
        ("region", "!=", "eu", Box::new(|r| r[1] != "eu")),
        (
            "status",
            "~",
            "FAIL",
            Box::new(|r| r[5].to_lowercase().contains("fail")),
        ),
        ("status", "!~", "fail", Box::new(|r| !r[5].contains("fail"))),
        (
            "amount",
            ">",
            "400",
            Box::new(|r| r[2].parse::<f64>().unwrap() > 400.0),
        ),
        (
            "amount",
            ">=",
            "400",
            Box::new(|r| r[2].parse::<f64>().unwrap() >= 400.0),
        ),
        (
            "amount",
            "<",
            "10",
            Box::new(|r| r[2].parse::<f64>().unwrap() < 10.0),
        ),
        (
            "amount",
            "<=",
            "10",
            Box::new(|r| r[2].parse::<f64>().unwrap() <= 10.0),
        ),
        // numbers compare as numbers: 9 < 10, and a placeholder is never "greater than 5"
        (
            "age",
            ">",
            "50",
            Box::new(|r| r[3].parse::<f64>().is_ok_and(|v| v > 50.0)),
        ),
        // a formatted number equals its plain spelling
        (
            "price",
            "=",
            "1013",
            Box::new(|r| r[4].replace(['$', ','], "").parse::<f64>().unwrap() == 1013.0),
        ),
    ];
    for (col, op, val, pred) in &cases {
        let want = rows.iter().filter(|r| pred(r)).count();
        let (code, out, _) = dtk(&home, &["where", file, col, op, val, "500"]);
        assert_eq!(code, 0, "{col} {op} {val}");
        assert!(
            out.contains(&format!("{want} of 300 rows match")),
            "{col} {op} {val}: want {want}:\n{out}"
        );
    }
}

#[test]
fn stats_gives_the_summary_lines_of_those_columns() {
    let (home, f, _) = fixture("stats");
    let (code, out, _) = dtk(
        &home,
        &["stats", f.to_str().unwrap(), "age", "price", "REGION"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("age: 0..89 mean "), "{out}");
    assert!(out.contains("30 placeholders excluded (N/A 30)"), "{out}");
    assert!(
        out.contains("price: ") && out.contains("written with currency"),
        "{out}"
    );
    assert!(
        out.contains("region: 3 distinct: "),
        "column names ignore case: {out}"
    );
}

#[test]
fn a_short_id_names_a_file_in_the_store() {
    let (home, f, rows) = fixture("id");
    let store = home.join(".local/share/dtk/store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::copy(&f, store.join("abcdef012345.csv")).unwrap();
    std::fs::write(store.join("abcdef012345.json"), "{\"a\": [1, 2, 3]}").unwrap();
    // the table is preferred to the raw copy that has the same id
    let (code, out, _) = dtk(&home, &["rows", "abcdef012345", "0", "2"]);
    assert_eq!(code, 0);
    assert_eq!(&parse(&out)[1..], &rows[..2]);
    // a prefix is enough
    let (code, _, _) = dtk(&home, &["rows", "abcdef", "0", "1"]);
    assert_eq!(code, 0);
    // the JSON copy for `get`
    let (code, out, err) = dtk(&home, &["get", "abcdef012345", ".a[1]"]);
    assert_eq!((code, out.trim()), (0, "2"), "{err}");
}

#[test]
fn mistakes_get_a_clear_message_and_a_failing_exit_code() {
    let (home, f, _) = fixture("errors");
    let file = f.to_str().unwrap();
    let (code, _, err) = dtk(&home, &["top", file, "nope"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("no column \"nope\". Columns: id, region, amount"),
        "{err}"
    );
    let (code, _, err) = dtk(&home, &["where", file, "region", "??", "x"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("is not an operator") && err.contains("dtk where"),
        "{err}"
    );
    let (code, _, err) = dtk(&home, &["rows", "/no/such/file.csv", "0"]);
    assert_eq!(code, 1);
    assert!(err.contains("no file"), "{err}");
    let (code, _, err) = dtk(&home, &["top", file, "region"]);
    assert_eq!(code, 1);
    assert!(err.contains("has no numbers"), "{err}");
    let (code, out, _) = dtk(&home, &["rows", "--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("dtk where FILE COLUMN OP VALUE"), "{out}");
}

#[test]
fn get_reads_parts_of_a_json_document() {
    let home = temp_home("get");
    let doc = json!({
        "name": "app",
        "items": [{"id": 1, "tags": ["a", "b"]}, {"id": 2, "tags": []}, {"id": 3, "tags": ["c"]}],
        "odd key": {"x": null},
        "n": 5
    });
    let f = home.join("doc.json");
    std::fs::write(&f, doc.to_string()).unwrap();
    let file = f.to_str().unwrap();
    let get = |path: &str| -> Vec<Value> {
        let (code, out, err) = dtk(&home, &["get", file, path]);
        assert_eq!(code, 0, "{path}: {err}");
        out.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    assert_eq!(get(".name"), [json!("app")]);
    assert_eq!(get(".items[1].id"), [json!(2)]);
    assert_eq!(get(".items[-1].id"), [json!(3)]);
    assert_eq!(get(".items[].id"), [json!(1), json!(2), json!(3)]);
    assert_eq!(get(".items | length"), [json!(3)]);
    assert_eq!(get(".items[0].tags | length"), [json!(2)]);
    assert_eq!(get(". | keys"), [json!(["name", "items", "odd key", "n"])]);
    assert_eq!(get(".[\"odd key\"].x"), [Value::Null]);
    assert_eq!(get(".items[9]"), [Value::Null]);
    assert_eq!(get("items[0]"), [json!({"id": 1, "tags": ["a", "b"]})]);
    // JSON lines are an array of their lines
    let jl = home.join("rows.jsonl");
    std::fs::write(&jl, "{\"a\":1}\n{\"a\":2}\n").unwrap();
    let (_, out, _) = dtk(&home, &["get", jl.to_str().unwrap(), ".[1].a"]);
    assert_eq!(out.trim(), "2");
    // mistakes
    let (code, _, err) = dtk(&home, &["get", file, ".name.x"]);
    assert_eq!(code, 1);
    assert!(err.contains("cannot take key \"x\" of a string"), "{err}");
}

#[test]
fn the_hook_leaves_the_output_of_a_drill_down_alone() {
    let home = temp_home("hook");
    let rows: Vec<Vec<String>> = orders();
    let f = home.join("orders.csv");
    write_csv(&f, &rows);
    // The output of `dtk where ... 200` is 200 rows: a table that dtk would summarize again.
    let (_, out, _) = dtk(
        &home,
        &["where", f.to_str().unwrap(), "region", "!=", "zz", "200"],
    );
    let file = home.join("out.txt");
    std::fs::write(&file, &out).unwrap();
    for cmd in [
        "dtk where abcdef012345 region != zz 200",
        "/usr/local/bin/dtk top abcdef012345 amount 200",
        "cd x && dtk rows abcdef012345 0 200",
    ] {
        let (_, why, _) = dtk(
            &home,
            &["explain", file.to_str().unwrap(), "--command", cmd],
        );
        assert!(
            why.contains("PASS THROUGH") && why.contains("drill-down"),
            "`{cmd}` was summarized:\n{why}"
        );
    }
    let (_, why, _) = dtk(
        &home,
        &[
            "explain",
            file.to_str().unwrap(),
            "--command",
            "cat orders.csv",
        ],
    );
    assert!(why.contains("decision:"), "{why}");
}
