//! `dtk explain` (why was this output summarized, or not?) and `dtk verify` (does everything work?).
//!
//! Both call `hook::evaluate`, the function the hook itself uses, so they cannot disagree with it.

use crate::config::{self, Config};
use crate::doctor;
use crate::hook::{evaluate, is_live, Input, DATA_EXTS};
use dtk::lean::est_tokens;
use std::fs;
use std::io::Read;
use std::path::Path;

/// `dtk explain [FILE|-] [--command CMD] [--read] [--mcp TOOL] [--saved]`
pub fn explain(args: &[String]) -> i32 {
    let mut file: Option<&str> = None;
    let mut command: Option<&str> = None;
    let (mut read, mut saved) = (false, false);
    let mut mcp: Option<&str> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--command" => command = it.next().map(String::as_str),
            "--read" => read = true,
            "--mcp" => mcp = it.next().map(String::as_str),
            "--saved" => saved = true,
            other if file.is_none() => file = Some(other),
            other => {
                eprintln!("dtk explain: unexpected argument {other}");
                return 2;
            }
        }
    }
    let (text, path) = match file {
        Some(p) if p != "-" => match fs::read_to_string(p) {
            Ok(s) => (s, Path::new(p).canonicalize().ok()),
            Err(e) => {
                eprintln!("dtk: cannot read {p}: {e}");
                return 1;
            }
        },
        _ => {
            let mut s = String::new();
            if std::io::stdin().read_to_string(&mut s).is_err() {
                return 1;
            }
            (s, None)
        }
    };
    let tool = if mcp.is_some() {
        "MCP"
    } else if read {
        "Read"
    } else {
        "Bash"
    };
    let default_subject = match (&path, read) {
        (Some(p), _) => {
            if read {
                p.display().to_string()
            } else {
                format!("cat {}", p.display())
            }
        }
        _ => "-".to_string(),
    };
    let subject = mcp
        .map(String::from)
        .or_else(|| command.map(String::from))
        .unwrap_or(default_subject);
    let (cfg, cfg_error) = Config::load();
    let live = is_live();
    println!("tool:      {tool}");
    println!("subject:   {subject}");
    println!(
        "mode:      {}",
        if live {
            "live"
        } else {
            "shadow (logs only, changes nothing)"
        }
    );
    let cfg_path = config::path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    match (&cfg_error, config::path().is_some_and(|p| p.exists())) {
        (Some(e), _) => println!("config:    {cfg_path} is ignored: {e}"),
        (None, true) => println!("config:    {cfg_path}"),
        (None, false) => println!("config:    none (defaults)"),
    }
    if read {
        let ext = Path::new(&subject)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !DATA_EXTS.contains(&ext.as_str()) {
            println!("decision:  PASS THROUGH (a .{ext} file is not a data file type; Read is only checked for: {})", DATA_EXTS.join(", "));
            return 0;
        }
    }
    let v = evaluate(
        &Input {
            tool,
            subject: &subject,
            text: &text,
            file: path.as_deref(),
            persisted: saved,
        },
        &cfg,
        false,
    );
    match (&v.detected, &v.rendered) {
        (Some(d), Some(r)) => {
            if d.name == "json-document" {
                println!("detected:  {}, {} entries at the top", d.name, d.rows);
            } else {
                println!("detected:  {}, {} rows x {} cols", d.name, d.rows, d.cols);
            }
            println!(
                "view:      {}",
                if r.lossless {
                    "every row, as CSV"
                } else if d.name == "json-document" {
                    "outline of the document"
                } else {
                    "summary of all rows"
                }
            );
            println!(
                "tokens:    {} for the model without dtk -> {} with the view",
                v.baseline, v.after
            );
        }
        _ => println!("detected:  nothing"),
    }
    if v.applies {
        if live {
            println!("decision:  APPLY ({})", v.reason);
        } else {
            println!("decision:  WOULD APPLY in live mode ({}); the mode is shadow, so the output stays as it is", v.reason);
        }
    } else {
        println!("decision:  PASS THROUGH ({})", v.reason);
    }
    0
}

// ---- dtk verify -------------------------------------------------------------------------------

struct Sample {
    name: &'static str,
    format: &'static str,
    text: String,
}

fn samples() -> Vec<Sample> {
    let levels = ["info", "info", "warn", "error"];
    let mut out = Vec::new();
    let mut s = String::new();
    for i in 0..200 {
        s.push_str(&format!("{{\"ts\":\"2026-11-02T10:{:02}:{:02}Z\",\"level\":\"{}\",\"msg\":\"event {}\",\"ms\":{}}}\n", i / 60, i % 60, levels[i % 4], i, (i * 37) % 900));
    }
    out.push(Sample {
        name: "JSON lines",
        format: "jsonl",
        text: s,
    });
    let mut s = String::from("id,name,region,amount,ok\n");
    for i in 0..200 {
        s.push_str(&format!(
            "{},user_{},{},{}.5,{}\n",
            1000 + i,
            i % 7,
            ["us", "eu", "ap"][i % 3],
            i * 3,
            i % 2 == 0
        ));
    }
    out.push(Sample {
        name: "CSV",
        format: "csv",
        text: s,
    });
    let mut s = String::new();
    for i in 0..150 {
        s.push_str(&format!(
            "ts=2026-11-02T10:{:02}:{:02}Z level={} msg=\"request {} done\" ms={}\n",
            i / 60,
            i % 60,
            levels[i % 4],
            i,
            (i * 13) % 700
        ));
    }
    out.push(Sample {
        name: "logfmt",
        format: "logfmt",
        text: s,
    });
    let mut s = String::new();
    for i in 0..150 {
        s.push_str(&format!("203.0.113.{} - - [02/Nov/2026:10:{:02}:{:02} +0000] \"GET /p/{} HTTP/1.1\" {} {} \"-\" \"curl/8.4.0\"\n", i % 250, i / 60, i % 60, i, [200, 200, 404, 500][i % 4], 100 + i * 7));
    }
    out.push(Sample {
        name: "web access log",
        format: "access-log",
        text: s,
    });
    let mut s = String::from("shape: (300, 3)\n┌──────┬───────┬────────┐\n│ id   ┆ name  ┆ amount │\n│ ---  ┆ ---   ┆ ---    │\n│ i64  ┆ str   ┆ f64    │\n╞══════╪═══════╪════════╡\n");
    for i in 0..300 {
        s.push_str(&format!(
            "│ {:<4} ┆ user{} ┆ {:<6} │\n",
            i,
            i % 5,
            i as f64 * 1.5
        ));
    }
    s.push_str("└──────┴───────┴────────┘\n");
    out.push(Sample {
        name: "polars table",
        format: "table",
        text: s,
    });
    let items: Vec<String> = (0..300)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"name\":\"pod-{i}\",\"phase\":\"{}\"}}",
                ["Running", "Pending"][i % 2]
            )
        })
        .collect();
    out.push(Sample {
        name: "JSON object wrapping an array",
        format: "json-object",
        text: format!("{{\"kind\":\"List\",\"items\":[{}]}}", items.join(",")),
    });
    let mut s = String::new();
    for i in 0..300 {
        s.push_str(&format!(
            "web-{}  | {{\"level\":\"{}\",\"ms\":{}}}\n",
            i % 3,
            levels[i % 4],
            i
        ));
    }
    out.push(Sample {
        name: "prefixed JSON lines",
        format: "jsonl-prefixed",
        text: s,
    });
    // A CI log: timestamps, no level field, one failed step.
    let mut s = String::new();
    for i in 0..300 {
        let msg = if i == 250 {
            "##[error]Process completed with exit code 1.".to_string()
        } else {
            format!("   Compiling crate{i} v0.1.{}", i % 9)
        };
        s.push_str(&format!(
            "2026-11-02T02:{:02}:{:02}.123456Z {msg}\n",
            i / 60,
            i % 60
        ));
    }
    out.push(Sample {
        name: "timestamped text log",
        format: "text-log",
        text: s,
    });
    // ps aux: the last column is free text.
    let mut s = String::from(
        "USER               PID  %CPU %MEM      VSZ    RSS   TT  STAT STARTED      TIME COMMAND\n",
    );
    for i in 0..120 {
        s.push_str(&format!(
            "{:<14} {:>7} {:>5.1} {:>4.1} {:>8} {:>6}   ??  Ss    9:{:02}AM   0:{:02}.00 /usr/bin/tool{}\n",
            ["root", "dev"][i % 2],
            100 + i * 3,
            (i % 7) as f64 * 1.1,
            (i % 4) as f64 * 0.3,
            400_000 + i * 100,
            20_000 + i * 10,
            i % 60,
            i % 60,
            [" --a", "", " --b --c"][i % 3]
        ));
    }
    out.push(Sample {
        name: "ps aux",
        format: "table",
        text: s,
    });
    // A JSON document with no rows to count: a small array beside a big nested one.
    let resources: Vec<String> = (0..400)
        .map(|i| format!("{{\"address\":\"res.web[{i}]\",\"type\":\"vm\",\"size\":{i},\"tags\":{{\"env\":\"prod\"}}}}"))
        .collect();
    let outputs: Vec<String> = (0..12)
        .map(|i| format!("{{\"name\":\"out{i}\",\"value\":\"v{i}\"}}"))
        .collect();
    out.push(Sample {
        name: "JSON document",
        format: "json-document",
        text: format!(
            "{{\"version\":\"1\",\"planned\":{{\"root\":{{\"resources\":[{}]}}}},\"outputs\":[{}]}}",
            resources.join(","),
            outputs.join(",")
        ),
    });
    out
}

struct Tally {
    pass: usize,
    fail: usize,
}

impl Tally {
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            self.pass += 1;
            println!("PASS  {name}");
        } else {
            self.fail += 1;
            println!("FAIL  {name}: {detail}");
        }
    }
}

/// `dtk verify`: built-in checks of the detection and the decision, then the setup checks.
pub fn verify() -> i32 {
    let cfg = Config::default();
    let mut t = Tally { pass: 0, fail: 0 };
    std::env::remove_var("DTK_DISABLED");
    for s in samples() {
        let v = evaluate(
            &Input {
                tool: "Bash",
                subject: "cat data",
                text: &s.text,
                file: None,
                persisted: false,
            },
            &cfg,
            false,
        );
        let found = v.detected.as_ref().map(|d| d.name).unwrap_or("nothing");
        t.check(
            &format!("detects {} as {}", s.name, s.format),
            found == s.format,
            &format!("found {found}"),
        );
        let full = est_tokens(&s.text);
        t.check(
            &format!("{}: the view is under a third of the output", s.name),
            v.applies && v.after * 3 < full,
            &format!(
                "{} -> {} tokens, applies: {} ({})",
                full, v.after, v.applies, v.reason
            ),
        );
    }
    // Real-world columns: placeholders in a column of numbers, and numbers written as $1,234.56.
    let mut messy = String::from("id,age,price,note\n");
    for i in 0..120 {
        let age = if i % 6 == 0 {
            "N/A".to_string()
        } else {
            (20 + i % 50).to_string()
        };
        messy.push_str(&format!(
            "{i},{age},\"${},{:03}.00\",row {i}\n",
            1 + i % 9,
            (i * 7) % 1000
        ));
    }
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "cat people.csv",
            text: &messy,
            file: None,
            persisted: false,
        },
        &cfg,
        false,
    );
    let shown = v.rendered.as_ref().map_or("", |r| r.text.as_str());
    t.check(
        "counts placeholders and reads formatted numbers",
        shown.contains("age: 20..69 mean")
            && shown.contains("20 placeholders excluded (N/A 20)")
            && shown.contains("written with currency"),
        shown,
    );
    let prose = "The quick brown fox jumps over the lazy dog, and then it rests.\n".repeat(60);
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "cat notes",
            text: &prose,
            file: None,
            persisted: false,
        },
        &cfg,
        false,
    );
    t.check(
        "leaves prose alone",
        v.table.is_none(),
        "prose was taken for a table",
    );
    let code =
        "fn main() {\n    let a = f(1, 2, 3);\n    println!(\"{}\", g(a, b));\n}\n".repeat(60);
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "cat main.rs",
            text: &code,
            file: None,
            persisted: false,
        },
        &cfg,
        false,
    );
    t.check(
        "leaves source code alone",
        v.table.is_none(),
        "code was taken for a table",
    );
    let csv = &samples()[1].text;
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "head -n 100 data.csv",
            text: csv,
            file: None,
            persisted: false,
        },
        &cfg,
        false,
    );
    t.check(
        "lets head, tail, sed -n and qsv slice through",
        !v.applies && v.reason.contains("exact rows"),
        &v.reason,
    );
    let strict = Config {
        exclude_commands: vec!["^cat ".into()],
        ..Config::default()
    };
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "cat data.csv",
            text: csv,
            file: None,
            persisted: false,
        },
        &strict,
        false,
    );
    t.check(
        "honours exclude_commands",
        !v.applies && v.reason.contains("exclude_commands"),
        &v.reason,
    );
    let small: String = csv.lines().take(12).collect::<Vec<_>>().join("\n");
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "cat small.csv",
            text: &small,
            file: None,
            persisted: false,
        },
        &cfg,
        false,
    );
    t.check(
        "leaves small outputs alone",
        !v.applies,
        "a 12-line output was replaced",
    );
    let v = evaluate(
        &Input {
            tool: "Bash",
            subject: "cat big.csv",
            text: csv,
            file: Some(Path::new("/tmp/big.csv")),
            persisted: true,
        },
        &cfg,
        false,
    );
    t.check(
        "replaces the preview of a saved-to-file output",
        v.applies,
        &v.reason,
    );
    println!("\n{} of {} built-in checks passed", t.pass, t.pass + t.fail);
    println!("\nsetup:");
    let warnings = doctor::run();
    if t.fail == 0 && warnings == 0 {
        println!("\nall good");
        0
    } else {
        1
    }
}
