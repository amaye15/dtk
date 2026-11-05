//! The words of a request, for finding the rows that a model is looking for.
//!
//! A model that asks for the status of order 5042 and gets a summary of 300 orders must take one
//! more step to see that row. The words of its request are known to the hook: the command, the
//! description that the model wrote for it, and the last thing the user typed. A row that holds
//! one of those words as a whole cell is very likely the row that is wanted, so the summary can
//! show it. Nothing here guesses: a word is only a key, and `lean` shows a row only when a whole
//! cell is that key, in a column where that can mean a row (see `lean::request_rows`).
//!
//! This is counting and comparing, with no model. The words never leave the machine and are not
//! stored.

use serde_json::Value;

/// Words that name nothing in a table: English, the words of a request, and the words of SQL,
/// the shell and Python. A cell can still equal one of them; it is only that a request is full of
/// them, and they would match many rows.
const STOP: &[&str] = &[
    "the", "and", "are", "was", "were", "you", "your", "for", "from", "with", "what", "which",
    "who", "whom", "whose", "when", "where", "why", "how", "give", "show", "tell", "find", "get",
    "list", "print", "run", "please", "then", "answer", "short", "line", "one", "bash", "command",
    "using", "use", "that", "this", "these", "those", "there", "here", "have", "has", "had",
    "does", "did", "can", "could", "would", "should", "any", "all", "each", "every", "row", "rows",
    "column", "columns", "table", "value", "values", "name", "number", "count", "many", "much",
    "most", "least", "largest", "smallest", "highest", "lowest", "not", "but", "its", "into",
    "out", "than", "also", "about", "after", "before", "only", "just", "like", "between", "select",
    "where", "group", "order", "limit", "offset", "join", "left", "right", "inner", "outer",
    "having", "distinct", "null", "asc", "desc", "true", "false", "insert", "update", "delete",
    "create", "drop", "python", "python3", "head", "tail", "grep", "cat", "echo", "sed", "awk",
    "import", "pandas", "data", "file",
];

/// The most keys that one request gives.
const MAX_KEYS: usize = 40;

/// The keys in `texts`: the words that could be the whole of a cell, lowercase, once each. A key
/// is at least three characters, is not a stop word, is not a flag or a path, and holds a letter
/// or a digit. `-`, `_`, `.`, `@` and `:` stay inside a key (`cust_011`, `us-east`, `a@b.org`).
pub fn keys(texts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in texts {
        let words = text.split(|c: char| c.is_whitespace() || "'\"`,;()[]{}<>=?!|&*#".contains(c));
        for raw in words {
            let w = raw.trim_matches(|c: char| !c.is_alphanumeric());
            if w.chars().count() < 3
                || raw.starts_with('-')
                || w.contains(['/', '\\'])
                || !w.chars().any(char::is_alphanumeric)
            {
                continue;
            }
            let w = w.to_lowercase();
            if STOP.contains(&w.as_str()) || out.contains(&w) {
                continue;
            }
            out.push(w);
            if out.len() >= MAX_KEYS {
                return out;
            }
        }
    }
    out
}

/// What the user last typed, from the end of a Claude Code transcript (JSON lines).
///
/// The line is a `user` entry whose content is text: not the result of a tool, not a note that
/// Claude Code added (those start with `<`), and not a subagent's prompt. The transcript is
/// Claude Code's own and is not documented, so this is a best effort: when it finds nothing, the
/// request is the command and its description, and nothing is wrong. The `last-prompt` entry is
/// not used: it was seen to hold the previous prompt while the new one was already a `user` line.
pub fn last_user_request(tail: &str) -> Option<String> {
    for line in tail.lines().rev() {
        if !line.contains("\"type\":\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "user" || v["isMeta"] == true || v["isSidechain"] == true {
            continue;
        }
        let text = match &v["message"]["content"] {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        let t = text.trim();
        if t.is_empty() || t.starts_with('<') {
            continue;
        }
        return Some(t.chars().take(2000).collect());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_the_words_that_could_be_a_whole_cell() {
        let k = keys(&["What is the status of order 5042 for cust_011 in us-east?"]);
        // `order` is a stop word (SQL), and a cell is almost never that word.
        assert_eq!(k, ["status", "5042", "cust_011", "us-east"]);
    }

    #[test]
    fn short_words_flags_paths_and_stop_words_are_not_keys() {
        let k = keys(&["python3 report.py --limit 100 /tmp/data.csv SELECT * FROM t WHERE id = 7"]);
        // `report.py` stays: a cell can be a file name. The rest is the request's own words.
        assert_eq!(k, ["report.py", "100"]);
    }

    #[test]
    fn a_key_is_kept_once_and_in_the_order_it_came() {
        assert_eq!(keys(&["Ann ann ANN", "Bob ann"]), ["ann", "bob"]);
    }

    #[test]
    fn an_address_and_a_date_stay_whole() {
        let k = keys(&["mail a@b.org on 2025-03-01."]);
        assert_eq!(k, ["mail", "a@b.org", "2025-03-01"]);
    }

    #[test]
    fn there_is_a_limit() {
        let text: String = (0..200).map(|i| format!("word{i} ")).collect();
        assert_eq!(keys(&[&text]).len(), MAX_KEYS);
    }

    fn line(kind: &str, content: Value, extra: &str) -> String {
        format!(
            "{{\"type\":\"{kind}\",\"message\":{{\"role\":\"user\",\"content\":{content}}}{extra}}}"
        )
    }

    #[test]
    fn the_last_thing_the_user_typed_is_found() {
        let t = [
            line("user", Value::String("first question".into()), ""),
            line("assistant", Value::String("an answer".into()), ""),
            line(
                "user",
                Value::String("what is the status of order 5042?".into()),
                "",
            ),
            // A tool result is a user entry too, with a list of blocks.
            line(
                "user",
                serde_json::json!([{"type": "tool_result", "content": "rows"}]),
                "",
            ),
            "{\"type\":\"last-prompt\",\"lastPrompt\":\"first question\"}".to_string(),
        ]
        .join("\n");
        assert_eq!(
            last_user_request(&t).as_deref(),
            Some("what is the status of order 5042?")
        );
    }

    #[test]
    fn notes_from_claude_code_meta_lines_and_subagents_are_not_requests() {
        let t = [
            line("user", Value::String("the real request".into()), ""),
            line("user", Value::String("<system-reminder>be brief</system-reminder>".into()), ""),
            line("user", Value::String("meta note".into()), ",\"isMeta\":true"),
            line("user", Value::String("a subagent was told this".into()), ",\"isSidechain\":true"),
            line("user", serde_json::json!([{"type": "text", "text": "<command-name>/clear</command-name>"}]), ""),
        ]
        .join("\n");
        assert_eq!(last_user_request(&t).as_deref(), Some("the real request"));
    }

    #[test]
    fn text_blocks_count_and_a_broken_first_line_is_skipped() {
        let t = format!(
            "ess\":\"half a line\"}}\n{}",
            line(
                "user",
                serde_json::json!([{"type": "text", "text": "look up cust_011"}]),
                ""
            )
        );
        assert_eq!(last_user_request(&t).as_deref(), Some("look up cust_011"));
        assert_eq!(last_user_request("not json at all\n"), None);
        assert_eq!(last_user_request(""), None);
    }
}
