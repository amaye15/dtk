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

fn sh(script: &str) -> String {
    let o = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{script}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn column<'a>(t: &'a dtk::table::Table, info: &'a [ColInfo], name: &str) -> &'a ColInfo {
    &info[t
        .headers
        .iter()
        .position(|h| h == name)
        .unwrap_or_else(|| panic!("no column {name}: {:?}", t.headers))]
}

fn load(file: &str) -> (dtk::table::Table, Vec<ColInfo>) {
    let t = detect(&std::fs::read_to_string(corpus(file)).unwrap())
        .unwrap_or_else(|| panic!("{file}: not detected"));
    let info = analyze(&t);
    (t, info)
}

#[test]
fn wrapped_json_matches_jaq() {
    if !have("jaq") {
        eprintln!("SKIPPED: jaq is not installed");
        return;
    }
    for (file, array) in [
        ("positive/gh_search_items.json", ".items"),
        ("positive/wrapped_items.json", ".items"),
    ] {
        let (t, info) = load(file);
        let p = corpus(file);
        let p = p.to_str().unwrap();
        let rows: usize = out("jaq", &[&format!("{array} | length"), p])
            .trim()
            .parse()
            .unwrap();
        assert_eq!(t.rows.len(), rows, "{file}: rows");
        let col = if file.contains("gh_search") {
            "stargazers_count"
        } else {
            "restarts"
        };
        let num = |f: &str| -> f64 {
            out("jaq", &[&format!("[{array}[].{col}] | {f}"), p])
                .trim()
                .parse()
                .unwrap()
        };
        let c = column(&t, &info, col);
        assert!(
            close(c.stats.min.unwrap(), num("min"), 1e-9),
            "{file}.{col}: min"
        );
        assert!(
            close(c.stats.max.unwrap(), num("max"), 1e-9),
            "{file}.{col}: max"
        );
        assert!(
            close(c.stats.mean.unwrap(), num("add / length"), 1e-9),
            "{file}.{col}: mean"
        );
    }
    // The scalar next to the array is kept as a note.
    let (t, _) = load("positive/gh_search_items.json");
    assert!(
        t.notes.iter().any(|n| n.starts_with("total_count: ")),
        "{:?}",
        t.notes
    );
}

#[test]
fn keyed_json_matches_jaq() {
    if !have("jaq") {
        eprintln!("SKIPPED: jaq is not installed");
        return;
    }
    let (t, info) = load("positive/keyed_by_id.json");
    let p = corpus("positive/keyed_by_id.json");
    let p = p.to_str().unwrap();
    assert_eq!(t.rows.len().to_string(), out("jaq", &["length", p]).trim());
    let num = |f: &str| -> f64 {
        out("jaq", &[&format!("[.[].age] | {f}"), p])
            .trim()
            .parse()
            .unwrap()
    };
    let age = column(&t, &info, "age");
    assert!(
        close(age.stats.min.unwrap(), num("min"), 1e-9)
            && close(age.stats.max.unwrap(), num("max"), 1e-9)
    );
    assert!(close(age.stats.mean.unwrap(), num("add / length"), 1e-9));
    // Every id is a row key.
    let keys = out("jaq", &["-r", "keys[]", p]);
    let ours: Vec<&str> = t.rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(keys.lines().collect::<Vec<_>>(), ours);
}

#[test]
fn logfmt_matches_grep_and_awk() {
    let file = "positive/logfmt_service.log";
    let (t, info) = load(file);
    let p = corpus(file);
    let p = p.to_str().unwrap();
    let lines: usize = sh(&format!("grep -c . '{p}'")).trim().parse().unwrap();
    assert_eq!(t.rows.len(), lines, "rows");
    for level in ["info", "warn", "error", "debug"] {
        let n: usize = sh(&format!("grep -c ' level={level} ' '{p}'"))
            .trim()
            .parse()
            .unwrap();
        let ours = column(&t, &info, "level")
            .stats
            .top
            .iter()
            .find(|(v, _)| v == level)
            .map_or(0, |(_, c)| *c);
        assert_eq!(ours, n, "lines with level={level}");
    }
    let mean: f64 = sh(&format!(
        "awk '{{for(i=1;i<=NF;i++) if($i ~ /^latency_ms=/){{split($i,a,\"=\"); s+=a[2]; n++}}}} END{{printf \"%.6f\", s/n}}' '{p}'"
    ))
    .trim()
    .parse()
    .unwrap();
    assert!(
        close(
            column(&t, &info, "latency_ms").stats.mean.unwrap(),
            mean,
            1e-6
        ),
        "latency_ms mean"
    );
    // A quoted value that holds a space and an escaped quote survives intact.
    assert!(column(&t, &info, "msg")
        .stats
        .top
        .iter()
        .any(|(v, _)| v == "user said \"hi there\""));
}

#[test]
fn access_log_matches_awk() {
    for (file, cols) in [
        ("positive/access_combined.log", 10),
        ("positive/access_common.log", 8),
    ] {
        let (t, info) = load(file);
        assert_eq!(t.headers.len(), cols, "{file}: columns");
        let p = corpus(file);
        let p = p.to_str().unwrap();
        assert_eq!(
            t.rows.len().to_string(),
            sh(&format!("grep -c . '{p}'")).trim(),
            "{file}: rows"
        );
        // Status code counts: the field right after the quoted request.
        let by_status = sh(&format!(
            "awk -F'\"' '{{split($3,a,\" \"); c[a[1]]++}} END{{for(k in c) print k, c[k]}}' '{p}'"
        ));
        let theirs: HashMap<String, usize> = by_status
            .lines()
            .map(|l| {
                let (k, v) = l.split_once(' ').unwrap();
                (k.to_string(), v.parse().unwrap())
            })
            .collect();
        let ours: HashMap<String, usize> = column(&t, &info, "status")
            .stats
            .top
            .iter()
            .cloned()
            .collect();
        assert_eq!(ours, theirs, "{file}: status counts");
        // Mean of the bytes that are numbers ("-" is no value).
        let mean: f64 = sh(&format!(
            "awk -F'\"' '{{split($3,a,\" \"); if (a[2] != \"-\") {{s+=a[2]; n++}}}} END{{printf \"%.6f\", s/n}}' '{p}'"
        ))
        .trim()
        .parse()
        .unwrap();
        let bytes = column(&t, &info, "bytes");
        assert_eq!(
            bytes.kind,
            Kind::Int,
            "{file}: bytes is a number once '-' is no value"
        );
        assert!(
            close(bytes.stats.mean.unwrap(), mean, 1e-6),
            "{file}: bytes mean"
        );
    }
}

#[test]
fn prefixed_jsonl_matches_sed_and_jaq() {
    if !have("jaq") {
        eprintln!("SKIPPED: jaq is not installed");
        return;
    }
    let file = "positive/docker_compose_logs.txt";
    let (t, info) = load(file);
    let p = corpus(file);
    let p = p.to_str().unwrap();
    let strip = format!("sed 's/^[^{{]*//' '{p}'");
    let mean: f64 = sh(&format!("{strip} | jaq -s '[.[].ms] | add / length'"))
        .trim()
        .parse()
        .unwrap();
    assert!(
        close(column(&t, &info, "ms").stats.mean.unwrap(), mean, 1e-9),
        "ms mean"
    );
    let services = sh(&format!("sed 's/ *|.*//' '{p}' | sort | uniq -c"));
    for l in services.lines() {
        let mut it = l.split_whitespace();
        let (n, name): (usize, &str) = (it.next().unwrap().parse().unwrap(), it.next().unwrap());
        let ours = column(&t, &info, "source")
            .stats
            .top
            .iter()
            .find(|(v, _)| v == name)
            .map_or(0, |(_, c)| *c);
        assert_eq!(ours, n, "lines from {name}");
    }
}
