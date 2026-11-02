//! Gate G4: dtk's numbers against independent tools.
//!
//! `qsv` and `jaq` compute the same statistics from the same files, with their own code. If dtk
//! says a column's mean is 80.7 and qsv says 80.7, that is evidence; if they differ, one is wrong.
//! Skipped (with a message) when the tools are not installed.

use dtk::lean::{analyze, ColInfo, Kind};
use dtk::table::detect;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn have(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn out(cmd: &str, args: &[&str]) -> String {
    let o = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{cmd} {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn corpus(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(file)
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-9)
}

fn records(csv_text: &str) -> Vec<HashMap<String, String>> {
    let mut rdr = csv::Reader::from_reader(csv_text.as_bytes());
    let headers = rdr.headers().unwrap().clone();
    rdr.records()
        .map(|r| {
            let r = r.unwrap();
            headers
                .iter()
                .zip(r.iter())
                .map(|(h, v)| (h.to_string(), v.to_string()))
                .collect()
        })
        .collect()
}

#[test]
fn csv_statistics_match_qsv() {
    if !have("qsv") {
        eprintln!("SKIPPED: qsv is not installed");
        return;
    }
    for file in [
        "positive/csv_orders.csv",
        "positive/csv_quoted.csv",
        "positive/sqlite_csv.csv",
    ] {
        let path = corpus(file);
        let p = path.to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let t = detect(&text).unwrap_or_else(|| panic!("{file}: not detected"));
        let info = analyze(&t);
        let rows: usize = out("qsv", &["count", p]).trim().parse().unwrap();
        assert_eq!(t.rows.len(), rows, "{file}: row count");
        let stats = records(&out("qsv", &["stats", "--everything", "--infer-dates", p]));
        assert_eq!(stats.len(), t.headers.len(), "{file}: column count");
        for (h, ci) in t.headers.iter().zip(&info) {
            let q = stats
                .iter()
                .find(|r| &r["field"] == h)
                .unwrap_or_else(|| panic!("{file}: qsv has no column {h}"));
            let qtype = q["type"].as_str();
            let ours = ci.kind;
            let ok = match qtype {
                "Integer" => ours == Kind::Int,
                "Float" => ours == Kind::Float,
                "String" => ours == Kind::Str,
                "Date" | "DateTime" => ours == Kind::Time,
                "Boolean" => ours == Kind::Bool,
                "NULL" => ours == Kind::Empty,
                _ => true,
            };
            assert!(ok, "{file}.{h}: qsv says {qtype}, dtk says {ours:?}");
            assert_eq!(
                ci.stats.nulls,
                q["nullcount"].parse::<usize>().unwrap(),
                "{file}.{h}: nulls"
            );
            if matches!(ours, Kind::Int | Kind::Float) {
                let get = |k: &str| q[k].parse::<f64>().unwrap();
                assert!(
                    close(ci.stats.min.unwrap(), get("min"), 1e-6),
                    "{file}.{h}: min {:?} vs {}",
                    ci.stats.min,
                    get("min")
                );
                assert!(
                    close(ci.stats.max.unwrap(), get("max"), 1e-6),
                    "{file}.{h}: max {:?} vs {}",
                    ci.stats.max,
                    get("max")
                );
                assert!(
                    close(ci.stats.mean.unwrap(), get("mean"), 1e-3),
                    "{file}.{h}: mean {:?} vs {}",
                    ci.stats.mean,
                    get("mean")
                );
                if let (Some(p50), Some(q50)) = (
                    ci.stats.p50,
                    q.get("q2_median").and_then(|v| v.parse::<f64>().ok()),
                ) {
                    let span = get("max") - get("min");
                    assert!(
                        (p50 - q50).abs() <= 0.02 * span.max(1e-9),
                        "{file}.{h}: p50 {p50} vs qsv median {q50}"
                    );
                }
            }
            if let (Some(d), Ok(card)) = (ci.distinct, q["cardinality"].parse::<usize>()) {
                assert_eq!(d, card, "{file}.{h}: distinct values");
            }
        }
    }
}

#[test]
fn category_counts_match_qsv_frequency() {
    if !have("qsv") {
        eprintln!("SKIPPED: qsv is not installed");
        return;
    }
    let path = corpus("positive/csv_orders.csv");
    let p = path.to_str().unwrap();
    let t = detect(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let info = analyze(&t);
    let freq = records(&out(
        "qsv",
        &["frequency", "--select", "region,status", "--limit", "0", p],
    ));
    for col in ["region", "status"] {
        let i = t.headers.iter().position(|h| h == col).unwrap();
        let theirs: HashMap<String, usize> = freq
            .iter()
            .filter(|r| r["field"] == col)
            .map(|r| (r["value"].clone(), r["count"].parse().unwrap()))
            .collect();
        let ours: HashMap<String, usize> = info[i].stats.top.iter().cloned().collect();
        assert_eq!(ours, theirs, "{col}: value counts");
    }
}

fn jaq_num(file: &str, filter: &str) -> f64 {
    out("jaq", &["-s", filter, corpus(file).to_str().unwrap()])
        .trim()
        .parse()
        .unwrap()
}

fn check_json_file(file: &str, slurped: bool) {
    let path = corpus(file);
    let t = detect(&std::fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|| panic!("{file}: not detected"));
    let info: Vec<ColInfo> = analyze(&t);
    let col = |name: &str| t.headers.iter().position(|h| h == name).unwrap();
    // `-s` slurps JSONL into an array; a JSON array file arrives as [[...]], so unwrap it.
    let arr = if slurped { "." } else { ".[0]" };
    let rows = jaq_num(file, &format!("{arr} | length"));
    assert_eq!(t.rows.len() as f64, rows, "{file}: rows");
    for name in ["amount", "latency_ms"] {
        let ci = &info[col(name)];
        let min = jaq_num(file, &format!("[{arr}[].{name}] | min"));
        let max = jaq_num(file, &format!("[{arr}[].{name}] | max"));
        let mean = jaq_num(file, &format!("[{arr}[].{name}] | add / length"));
        assert!(
            close(ci.stats.min.unwrap(), min, 1e-9),
            "{file}.{name}: min"
        );
        assert!(
            close(ci.stats.max.unwrap(), max, 1e-9),
            "{file}.{name}: max"
        );
        assert!(
            close(ci.stats.mean.unwrap(), mean, 1e-9),
            "{file}.{name}: mean {:?} vs {mean}",
            ci.stats.mean
        );
    }
    let by_region = out(
        "jaq",
        &[
            "-s",
            "-c",
            &format!("[{arr}[].region] | group_by(.) | map([.[0], length])"),
            corpus(file).to_str().unwrap(),
        ],
    );
    let theirs: Vec<(String, usize)> = serde_json::from_str(&by_region).unwrap();
    let ours: HashMap<String, usize> = info[col("region")].stats.top.iter().cloned().collect();
    assert_eq!(ours.len(), theirs.len(), "{file}: region values");
    for (v, n) in theirs {
        assert_eq!(ours.get(&v), Some(&n), "{file}: count of region {v}");
    }
}

#[test]
fn json_statistics_match_jaq() {
    if !have("jaq") {
        eprintln!("SKIPPED: jaq is not installed");
        return;
    }
    check_json_file("positive/jsonl_orders.jsonl", true);
    check_json_file("positive/json_array_orders.json", false);
}
