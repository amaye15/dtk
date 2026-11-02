//! What dtk needs to know about rtk's own rewrites and output.
//!
//! rtk's PreToolUse hook changes the command before it runs (`cat f` becomes `rtk read f`,
//! `head -n 5 f` becomes `rtk read f --head-lines 5`), and the PostToolUse event that dtk gets
//! carries the changed command. Two things follow:
//!
//! - dtk must see through the rewrite when it checks what the user asked for and what they
//!   excluded in `config.toml`.
//! - rtk may already have cut the output. A table with a hole in it is not "all rows", so dtk
//!   must leave it alone, and rtk's pointer to the rest must stay in front of the model.

/// The command as the user typed it, before rtk rewrote it. Only the words that matter to dtk's
/// checks are restored: `rtk` is dropped from the front of each command in a pipeline, and
/// `rtk read` becomes `cat`.
pub fn plain_command(cmd: &str) -> String {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut at_command = true;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if at_command && w == "rtk" {
            i += 1;
            // Global flags such as `rtk -u grep` come before the subcommand.
            while i < words.len() && words[i].starts_with('-') {
                i += 1;
            }
            match words.get(i) {
                Some(&"read") => {
                    out.push("cat");
                    i += 1;
                }
                Some(&"proxy") => i += 1,
                _ => {}
            }
            at_command = false;
            continue;
        }
        out.push(w);
        at_command = matches!(w, "|" | "||" | "&&" | ";" | "&") || (at_command && is_assignment(w));
        i += 1;
    }
    out.join(" ")
}

/// `NAME=value`, as in `PGPASSWORD=x psql ...`.
fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// The line in which rtk says it cut the output, if there is one. rtk 1.1.0 writes these:
/// `[full output: ...]`, `[see remaining: ...]`, `[+N hidden: rtk recall HASH]`, `... (N lines
/// truncated)`, `... (N lines omitted)`, `+N more in FILE`, `--- truncated at N`, and
/// `[RTK:PASSTHROUGH] Output truncated`.
pub fn truncation_marker(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| is_marker(l))
}

fn is_marker(l: &str) -> bool {
    if l.starts_with("[full output:")
        || l.starts_with("[see remaining:")
        || l.starts_with("[RTK:PASSTHROUGH]")
        || l.starts_with("--- truncated at")
        || l.starts_with("... (more changes truncated)")
    {
        return true;
    }
    // `[+9962 hidden: rtk recall 8622e55af3d431a7]`, the pointer when rtk keeps the rest in its
    // recall store and not in a tee file.
    if l.starts_with("[+") && l.contains(" hidden:") && l.ends_with(']') {
        return true;
    }
    if l.starts_with('[') && l.ends_with(']') && l.contains("rtk recall ") {
        return true;
    }
    if l.starts_with("...") {
        let tail = l.trim_end();
        if [
            " lines truncated)",
            " lines omitted)",
            " lines omitted]",
            "output truncated)",
        ]
        .iter()
        .any(|t| tail.ends_with(t))
        {
            return true;
        }
        if tail.starts_with("... (+") && tail.ends_with(')') {
            return true;
        }
    }
    // `+725 more in big.csv [see remaining: ...]`
    if let Some(rest) = l.strip_prefix('+') {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(" more in ") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_what_rtk_rewrote() {
        assert_eq!(plain_command("rtk read big.csv"), "cat big.csv");
        assert_eq!(
            plain_command("rtk read big.csv --head-lines 400"),
            "cat big.csv --head-lines 400"
        );
        assert_eq!(
            plain_command("rtk grep failed big.csv"),
            "grep failed big.csv"
        );
        assert_eq!(plain_command("rtk -u jq . f.json"), "jq . f.json");
        assert_eq!(plain_command("rtk proxy git log"), "git log");
        assert_eq!(
            plain_command("rtk read a.csv | rtk wc -l"),
            "cat a.csv | wc -l"
        );
        assert_eq!(
            plain_command("PGPASSWORD=x rtk psql -c 'select 1'"),
            "PGPASSWORD=x psql -c 'select 1'"
        );
        assert_eq!(plain_command("cd d && rtk curl -s u"), "cd d && curl -s u");
    }

    #[test]
    fn leaves_other_commands_alone() {
        for c in [
            "cat data.csv",
            "echo rtk read",
            "grep rtk file",
            "cargo install rtk",
            "qsv stats rtk.csv",
        ] {
            assert_eq!(plain_command(c), c, "{c}");
        }
    }

    #[test]
    fn finds_every_wording_of_rtk_1_1_0() {
        for m in [
            "... (17962 lines truncated)",
            "[see remaining: tail -n +41 \"/Users/x/Library/Application Support/rtk/tee/1_jq.log\"]",
            "[full output: rtk recall ab12cd]",
            "  +725 more in big.csv [see remaining: tail -n +26 \"/x/tee/1.log\"]",
            "... (12 lines omitted)",
            "... (cut: 40 lines omitted]",
            "... (output truncated)",
            "... (+3 more files)",
            "--- truncated at 20000 bytes",
            "[RTK:PASSTHROUGH] Output truncated (1.2 MB)",
            "... (more changes truncated)",
            "[+9962 hidden: rtk recall 8622e55af3d431a7]",
            "[output cut; rtk recall 8622e55af3d431a7 shows all]",
        ] {
            assert!(truncation_marker(m).is_some(), "missed: {m}");
        }
    }

    #[test]
    fn ignores_ordinary_data() {
        let table =
            "id,name,note\n1,a,+1 more\n2,b,... and so on\n3,c,[x] done\n4,d,see remaining\n";
        assert!(truncation_marker(table).is_none());
        assert!(truncation_marker("+12,34,56\n+7 more\n").is_none());
        assert!(truncation_marker("...\n... (see below)\n").is_none());
    }

    #[test]
    fn finds_a_marker_after_the_rows() {
        let text =
            "a,b\n1,2\n3,4\n... (4400 lines truncated)\n[see remaining: tail -n +4 \"/t/1.log\"]\n";
        assert_eq!(truncation_marker(text), Some("... (4400 lines truncated)"));
    }
}
