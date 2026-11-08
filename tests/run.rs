//! `dtk run` and `dtk filter`: dtk for an agent that cannot replace a tool's output, only rewrite
//! its command or pipe it. What matters most is what they must not change: an output that dtk
//! leaves alone comes out byte for byte as it was, and the exit code of the command is kept.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_dtk");

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dtk-run-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".claude")).unwrap();
    dir.canonicalize().unwrap()
}

fn dtk(home: &Path, args: &[&str], stdin: &[u8], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DTK_MODE")
        .env_remove("DTK_DISABLED")
        .env_remove("DTK_REQUEST");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

fn orders() -> String {
    let mut s = String::from("order_id,customer,region,amount,status\n");
    for i in 0..300 {
        s.push_str(&format!(
            "{},cust_{:03},{},{}.5,{}\n",
            1000 + i,
            i % 40,
            ["us", "eu", "ap"][i % 3],
            i * 3,
            ["ok", "ok", "failed"][i % 3]
        ));
    }
    s
}

fn file(home: &Path, name: &str, text: &str) -> String {
    let p = home.join(name);
    fs::write(&p, text).unwrap();
    p.display().to_string()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

#[test]
fn a_command_that_prints_a_table_is_run_and_summarized() {
    let h = home("table");
    let f = file(&h, "orders.csv", &orders());
    let o = dtk(&h, &["run", "-c", &format!("cat {f}")], b"", &[]);
    assert_eq!(o.status.code(), Some(0));
    let s = text(&o);
    assert!(s.starts_with("[dtk] csv: 300 rows x 5 cols"), "{s}");
    assert!(
        s.len() * 4 < orders().len(),
        "{} -> {}",
        orders().len(),
        s.len()
    );
    // The argument form does the same.
    let o = dtk(&h, &["run", "--", "cat", &f], b"", &[]);
    assert!(text(&o).starts_with("[dtk] csv: 300 rows"), "{}", text(&o));
}

#[test]
fn what_dtk_leaves_alone_comes_out_byte_for_byte() {
    let h = home("exact");
    let f = file(&h, "orders.csv", &orders());
    let raw = orders();
    // The user asked for exactly these rows.
    let o = dtk(&h, &["run", "-c", &format!("head -n 120 {f}")], b"", &[]);
    let want: String = raw.lines().take(120).map(|l| format!("{l}\n")).collect();
    assert_eq!(text(&o), want);
    // Text that is not data, with and without a final newline.
    for t in ["hello world\n", "no newline at the end", ""] {
        let o = dtk(&h, &["run", "-c", &format!("printf '%s' '{t}'")], b"", &[]);
        assert_eq!(text(&o), t, "{t:?}");
    }
    // Output that is not text at all.
    let o = dtk(&h, &["run", "-c", "printf '\\377\\376\\000abc'"], b"", &[]);
    assert_eq!(o.stdout, [0xff, 0xfe, 0x00, b'a', b'b', b'c']);
    // The switch that turns dtk off.
    let o = dtk(
        &h,
        &["run", "-c", &format!("cat {f}")],
        b"",
        &[("DTK_DISABLED", "1")],
    );
    assert_eq!(text(&o), raw);
    // And the shadow mode, which changes nothing.
    let o = dtk(
        &h,
        &["run", "-c", &format!("cat {f}")],
        b"",
        &[("DTK_MODE", "shadow")],
    );
    assert_eq!(text(&o), raw);
}

#[test]
fn the_exit_code_is_the_commands_and_a_failed_command_shows_its_output_as_it_was() {
    let h = home("exit");
    let f = file(&h, "orders.csv", &orders());
    let o = dtk(&h, &["run", "-c", &format!("cat {f}; exit 3")], b"", &[]);
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(text(&o), orders(), "a failure is not summarized");
    let o = dtk(&h, &["run", "-c", "exit 0"], b"", &[]);
    assert_eq!(o.status.code(), Some(0));
    let o = dtk(&h, &["run", "-c", "kill -TERM $$"], b"", &[]);
    assert_eq!(
        o.status.code(),
        Some(128 + 15),
        "a signal is 128 and the signal"
    );
    let o = dtk(&h, &["run", "--", "/no/such/program"], b"", &[]);
    assert_eq!(o.status.code(), Some(127));
}

#[test]
fn stderr_goes_through_as_it_comes_and_standard_input_reaches_the_command() {
    let h = home("stderr");
    let f = file(&h, "orders.csv", &orders());
    let o = dtk(
        &h,
        &["run", "-c", &format!("echo 'a warning' >&2; cat {f}")],
        b"",
        &[],
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("a warning"));
    assert!(text(&o).starts_with("[dtk] csv: 300 rows"), "{}", text(&o));
    let o = dtk(&h, &["run", "-c", "cat"], b"from stdin\n", &[]);
    assert_eq!(text(&o), "from stdin\n");
}

#[test]
fn the_output_of_a_dtk_query_is_never_summarized() {
    let h = home("query");
    let f = file(&h, "orders.csv", &orders());
    let first = text(&dtk(&h, &["run", "-c", &format!("cat {f}")], b"", &[]));
    let id = first
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .and_then(|p| {
            Path::new(p.split(' ').next().unwrap())
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .expect("an id in the summary");
    // 150 rows are more than a summary would keep; the model asked for them.
    let o = dtk(
        &h,
        &["run", "-c", &format!("{BIN} rows {id} 0 150")],
        b"",
        &[],
    );
    let s = text(&o);
    assert!(!s.contains("[dtk]"), "{}", &s[..s.len().min(300)]);
    assert!(s.lines().count() >= 150, "{} lines", s.lines().count());
}

#[test]
fn the_words_of_the_request_are_given_with_request_or_the_environment() {
    let h = home("request");
    let f = file(&h, "orders.csv", &orders());
    for o in [
        dtk(
            &h,
            &[
                "run",
                "--request",
                "look up order 1150",
                "-c",
                &format!("cat {f}"),
            ],
            b"",
            &[],
        ),
        dtk(
            &h,
            &["run", "-c", &format!("cat {f}")],
            b"",
            &[("DTK_REQUEST", "look up order 1150")],
        ),
    ] {
        let s = text(&o);
        assert!(
            s.contains("matches for words in your request (order_id = 1150)"),
            "{s}"
        );
    }
}

#[test]
fn filter_summarizes_standard_input_and_passes_the_rest_through() {
    let h = home("filter");
    let o = dtk(
        &h,
        &["filter", "--command", "cat orders.csv"],
        orders().as_bytes(),
        &[],
    );
    assert!(
        text(&o).starts_with("[dtk] csv: 300 rows x 5 cols"),
        "{}",
        text(&o)
    );
    assert_eq!(o.status.code(), Some(0));
    let o = dtk(&h, &["filter"], b"just some text\nno table here\n", &[]);
    assert_eq!(text(&o), "just some text\nno table here\n");
    // `head` in the command says that the rows were asked for.
    let o = dtk(
        &h,
        &["filter", "--command", "head -n 100 orders.csv"],
        orders().as_bytes(),
        &[],
    );
    assert_eq!(text(&o), orders());
    let o = dtk(&h, &["filter", "--nonsense"], b"", &[]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn a_bad_call_is_refused_with_the_usage() {
    let h = home("usage");
    let o = dtk(&h, &["run"], b"", &[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("usage: dtk run"));
    let o = dtk(&h, &["run", "-c"], b"", &[]);
    assert_eq!(o.status.code(), Some(2));
}
