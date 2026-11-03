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

/// The most that is read back from rtk, in bytes.
const MAX_RECOVERED: u64 = 64 * 1024 * 1024;

/// The whole output, when rtk cut this one and kept the rest where dtk can read it.
///
/// rtk saves what it cuts: in a tee file (`[see remaining: tail -n +26 ".../rtk/tee/1_x.log"]`)
/// or in its recall store (`[+9962 hidden: rtk recall 8622e55af3d431a7]`). Data that rtk has cut
/// to fifty rows is what dtk is for, so dtk takes the complete output from there, and its summary
/// covers every row. Nothing is guessed: the complete output must begin as the cut one does, a
/// tee file must sit in rtk's own `rtk/tee` folder, and a copy that rtk itself truncated (it
/// keeps the first megabyte) is not used, because a summary of it would say "all rows" and be
/// wrong.
pub fn recover(shown: &str) -> Option<String> {
    // rtk may write several markers (`... (751 lines omitted)`, then the pointer to the rest).
    let markers = || shown.lines().map(str::trim).filter(|l| is_marker(l));
    let full = match markers().find_map(tee_path) {
        Some(path) => read_tee(&path)?,
        None => run_recall(&markers().find_map(recall_hash)?)?,
    };
    let first = |s: &str| {
        s.lines()
            .map(str::trim_end)
            .find(|l| !l.trim().is_empty())
            .map(str::to_string)
    };
    if full.len() <= shown.len() || first(&full)? != first(shown)? {
        return None;
    }
    Some(full)
}

/// `[see remaining: tail -n +26 "$HOME/Library/Application Support/rtk/tee/1_uv-run.log"]`
fn tee_path(marker: &str) -> Option<std::path::PathBuf> {
    let at = marker.find("tail -n +")?;
    let rest = &marker[at..];
    let (a, b) = (rest.find('"')?, rest.rfind('"')?);
    if b <= a {
        return None;
    }
    let p = &rest[a + 1..b];
    let expanded = match p
        .strip_prefix("${HOME}")
        .or_else(|| p.strip_prefix("$HOME"))
        .or_else(|| p.strip_prefix('~'))
    {
        Some(tail) => format!("{}{tail}", crate::hook::home()?.display()),
        None => p.to_string(),
    };
    // `$HOME/.local/share/...` after a `C:\Users\me` home: one kind of separator, or Windows
    // (a `\\?\` path in particular) does not find the file.
    #[cfg(windows)]
    let expanded = expanded.replace('/', "\\");
    Some(std::path::PathBuf::from(expanded))
}

fn read_tee(path: &std::path::Path) -> Option<String> {
    let real = path.canonicalize().ok()?;
    let parts: Vec<_> = real.components().map(|c| c.as_os_str()).collect();
    let in_rtks_tee = parts.windows(2).any(|w| w[0] == "rtk" && w[1] == "tee");
    if !in_rtks_tee || real.extension().map_or(true, |e| e != "log") {
        return None;
    }
    if std::fs::metadata(&real).ok()?.len() > MAX_RECOVERED {
        return None;
    }
    let text = std::fs::read_to_string(&real).ok()?;
    // rtk's own copy ends with this line when the output was more than it keeps.
    if text
        .lines()
        .rev()
        .take(3)
        .any(|l| l.starts_with("--- truncated at"))
    {
        return None;
    }
    Some(text)
}

/// `rtk recall 8622e55af3d431a7` in a marker: the hash.
fn recall_hash(marker: &str) -> Option<String> {
    let at = marker.find("rtk recall ")?;
    let hash: String = marker[at + "rtk recall ".len()..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    (4..=64).contains(&hash.len()).then_some(hash)
}

/// `rtk recall --full HASH`, with a time limit: a hook must answer fast.
fn run_recall(hash: &str) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new("rtk")
        .args(["recall", "--full", hash])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.by_ref().take(MAX_RECOVERED).read_to_end(&mut buf);
        buf
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if start.elapsed() > std::time::Duration::from_secs(3) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    let text = String::from_utf8(reader.join().ok()?).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_path_and_the_hash_from_rtks_pointers() {
        let p = tee_path("[see remaining: tail -n +26 \"/x/rtk/tee/1_uv-run-stdout.log\"]");
        assert_eq!(
            p.unwrap(),
            std::path::Path::new("/x/rtk/tee/1_uv-run-stdout.log")
        );
        assert!(tee_path("[full output: rtk recall ab12cd]").is_none());
        assert_eq!(
            recall_hash("[+9962 hidden: rtk recall 8622e55af3d431a7]").as_deref(),
            Some("8622e55af3d431a7")
        );
        assert_eq!(
            recall_hash("[full output: rtk recall ab12cd]").as_deref(),
            Some("ab12cd")
        );
        assert!(recall_hash("[full output: rtk recall nothex]").is_none());
        assert!(recall_hash("[see remaining: tail -n +4 \"/t/1.log\"]").is_none());
    }

    #[test]
    fn recovers_only_a_complete_copy_from_rtks_own_folder() {
        let root = std::env::temp_dir().join(format!("dtk-recover-{}", std::process::id()));
        let tee = root.join("rtk/tee");
        let other = root.join("other");
        std::fs::create_dir_all(&tee).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let full: String = std::iter::once("id,name\n".to_string())
            .chain((0..100).map(|i| format!("{i},user_{i}\n")))
            .collect();
        let shown = |path: &std::path::Path| {
            format!(
                "id,name\n0,user_0\n1,user_1\n... (97 lines omitted)\n[see remaining: tail -n +4 \"{}\"]\n",
                path.display()
            )
        };
        let good = tee.join("1_x.log");
        std::fs::write(&good, &full).unwrap();
        assert_eq!(recover(&shown(&good)).as_deref(), Some(full.as_str()));
        // Not in a folder named rtk/tee.
        let elsewhere = other.join("1_x.log");
        std::fs::write(&elsewhere, &full).unwrap();
        assert!(recover(&shown(&elsewhere)).is_none());
        // Not a .log file.
        let odd = tee.join("1_x.txt");
        std::fs::write(&odd, &full).unwrap();
        assert!(recover(&shown(&odd)).is_none());
        // rtk's own copy was cut at its size limit.
        let cut = tee.join("2_cut.log");
        std::fs::write(
            &cut,
            format!("{full}\n--- truncated at 1048576 bytes ---\n"),
        )
        .unwrap();
        assert!(recover(&shown(&cut)).is_none());
        // A copy of some other output.
        let stale = tee.join("3_stale.log");
        std::fs::write(&stale, "name,value\n1,a\n".repeat(60)).unwrap();
        assert!(recover(&shown(&stale)).is_none());
        // A file that is gone.
        assert!(recover(&shown(&tee.join("4_gone.log"))).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

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
