//! Output of the GitHub CLI (`gh`) when it is not on a terminal.
//!
//! Without a terminal `gh` prints plain tab-separated lines with no header, so there is nothing in
//! the text to say what a column is. The command says it: `gh run list` has the same nine fields
//! in every version, `gh pr list` the same five. A listing is read as a table only when the
//! command names one of these and the lines have that many fields, and the columns get the names
//! of the fields. `gh run view --log` prints `job`, `step` and then a timestamped line of the
//! job's log; that goes to the timestamped-log reader, with job and step as columns.
//!
//! The command is not a fact when the user shaped the output (`--json`, `--jq`, `--template`), so
//! nothing here applies then.

use crate::table::{valid_header, Format, Table, MIN_ROWS};

/// (noun, verb, the fields of one line, in order)
const LISTINGS: &[(&str, &str, &[&str])] = &[
    (
        "run",
        "list",
        &[
            "status",
            "conclusion",
            "title",
            "workflow",
            "branch",
            "event",
            "id",
            "elapsed",
            "created",
        ],
    ),
    (
        "pr",
        "list",
        &["number", "title", "branch", "state", "created"],
    ),
    (
        "issue",
        "list",
        &["number", "state", "title", "labels", "updated"],
    ),
    ("release", "list", &["title", "status", "tag", "published"]),
    (
        "repo",
        "list",
        &["name", "description", "visibility", "updated"],
    ),
    (
        "search",
        "repos",
        &["name", "description", "visibility", "updated"],
    ),
    (
        "search",
        "issues",
        &["repo", "number", "state", "title", "labels", "updated"],
    ),
    (
        "search",
        "prs",
        &["repo", "number", "state", "title", "labels", "updated"],
    ),
    (
        "pr",
        "checks",
        &["name", "state", "elapsed", "url", "description"],
    ),
    ("workflow", "list", &["name", "state", "id"]),
    ("label", "list", &["name", "description", "color"]),
];

/// The `gh NOUN VERB` of a command, and whether the output was shaped by the user.
fn gh_call(command: &str) -> Option<(String, String, bool)> {
    for segment in command.split(['|', ';', '&']) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let Some(at) = words
            .iter()
            .position(|w| matches!(w.rsplit(['/', '\\']).next(), Some("gh" | "gh.exe")))
        else {
            continue;
        };
        let rest = &words[at + 1..];
        let positional: Vec<&str> = rest
            .iter()
            .copied()
            .filter(|w| !w.starts_with('-'))
            .collect();
        if positional.len() < 2 {
            continue;
        }
        let shaped = rest.iter().any(|w| {
            matches!(*w, "--json" | "--jq" | "-q" | "--template" | "-t")
                || w.starts_with("--json=")
                || w.starts_with("--jq=")
                || w.starts_with("--template=")
        });
        return Some((positional[0].to_string(), positional[1].to_string(), shaped));
    }
    None
}

/// A table from the output of a `gh` command, or `None`.
pub fn parse(command: &str, text: &str) -> Option<Table> {
    let (noun, verb, shaped) = gh_call(command)?;
    if shaped {
        return None;
    }
    let text = crate::table::clean(text);
    if noun == "run" && verb == "view" {
        return parse_run_log(&text);
    }
    let (_, _, names) = LISTINGS.iter().find(|(n, v, _)| *n == noun && *v == verb)?;
    parse_listing(&text, names)
}

fn parse_listing(text: &str, names: &[&str]) -> Option<Table> {
    let lines: Vec<&str> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.len() < MIN_ROWS {
        return None;
    }
    let rows: Vec<Vec<String>> = lines
        .iter()
        .map(|l| {
            l.trim_end_matches('\r')
                .split('\t')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|r| r.len() == names.len())
        .collect();
    // Every line is a row: a line with another number of fields means the layout is not the one
    // this reader knows.
    if rows.len() != lines.len() {
        return None;
    }
    let headers: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    if !valid_header(&headers) {
        return None;
    }
    Some(Table {
        format: Format::Tsv,
        headers,
        rows,
        notes: Vec::new(),
    })
}

/// `job <TAB> step <TAB> 2026-11-03T06:36:38.0667054Z message`, one line of a job's log each.
fn parse_run_log(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut prefixes: Vec<(&str, &str)> = Vec::with_capacity(lines.len());
    let mut rests: Vec<&str> = Vec::with_capacity(lines.len());
    for l in &lines {
        // A message may hold a tab of its own, so the rest is everything after the second.
        let mut it = l.splitn(3, '\t');
        let (job, step, rest) = (it.next()?, it.next()?, it.next()?);
        prefixes.push((job, step));
        rests.push(rest.trim_start_matches('\u{feff}'));
    }
    let mut table = crate::logs::parse_text_log(&rests.join("\n"))?;
    // Each line must be one record, so that the job and the step of a row are those of its line.
    if table.rows.len() != lines.len() {
        return None;
    }
    for (row, (job, step)) in table.rows.iter_mut().zip(prefixes) {
        row.splice(0..0, [job.to_string(), step.to_string()]);
    }
    table
        .headers
        .splice(0..0, ["job".to_string(), "step".to_string()]);
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_gh_command_however_it_is_written() {
        let call = gh_call;
        assert_eq!(
            call("gh run list -R a/b --limit 100"),
            Some(("run".into(), "list".into(), false))
        );
        assert_eq!(
            call("cd x && /opt/homebrew/bin/gh pr list --state all"),
            Some(("pr".into(), "list".into(), false))
        );
        assert_eq!(
            call("rtk gh run list"),
            Some(("run".into(), "list".into(), false))
        );
        assert_eq!(
            call("gh run list --json databaseId"),
            Some(("run".into(), "list".into(), true))
        );
        assert_eq!(
            call("gh pr list --jq '.[]'"),
            Some(("pr".into(), "list".into(), true))
        );
        // The word `gh` has to be a program, not an argument.
        assert_eq!(call("echo gh-run list"), None);
        assert_eq!(call("ls -la"), None);
    }

    #[test]
    fn a_listing_gets_the_names_of_its_fields() {
        let text: String = (0..12)
            .map(|i| {
                format!(
                    "{i}\ttitle number {i}\tbranch-{i}\tMERGED\t2026-11-03T06:25:5{}Z\n",
                    i % 10
                )
            })
            .collect();
        let t = parse("gh pr list -R a/b --state all", &text).unwrap();
        assert_eq!(t.headers, ["number", "title", "branch", "state", "created"]);
        assert_eq!(t.rows.len(), 12);
        // Not the layout of this command: no table.
        assert!(parse("gh run list", &text).is_none());
        // The user shaped the output.
        assert!(parse("gh pr list --jq '.[].title'", &text).is_none());
        // Too few lines.
        assert!(parse(
            "gh pr list",
            &text.lines().take(5).collect::<Vec<_>>().join("\n")
        )
        .is_none());
    }

    #[test]
    fn a_job_log_keeps_the_job_and_the_step_of_each_line() {
        let mut text = String::new();
        for i in 0..60 {
            let step = ["Set up job", "Build", "Test"][i / 20];
            let msg = if i == 45 {
                "##[error]Process completed with exit code 1."
            } else {
                "running a step"
            };
            text.push_str(&format!(
                "test (ubuntu)\t{step}\t2026-11-03T06:36:{:02}.0000000Z {msg}\n",
                i % 60
            ));
        }
        let t = parse("gh run view 123 --log", &text).unwrap();
        assert_eq!(&t.headers[..3], ["job", "step", "ts"]);
        assert_eq!(t.rows.len(), 60);
        assert_eq!(t.rows[45][..2], ["test (ubuntu)", "Test"]);
        assert!(t.rows[45].iter().any(|c| c.contains("##[error]")));
        // A line with one tab is not a log line of this layout.
        assert!(parse("gh run view 123 --log", &format!("{text}broken\tline\n")).is_none());
    }
}
