//! `~/.config/dtk/config.toml`: what dtk leaves alone, and its thresholds.
//!
//! ```toml
//! [hooks]
//! # Leave these alone. "^text" matches the start of a command (or a file path for Read);
//! # any other text matches anywhere in it.
//! exclude_commands = ["^kubectl", "psql"]
//!
//! [thresholds]
//! min_tokens = 1200        # smallest output worth replacing (estimated tokens)
//! budget = 2500            # largest summary
//! min_persisted_rows = 30  # fewest rows for output that Claude Code saved to a file
//! min_outline_tokens = 4000  # smallest JSON document (not a table) worth an outline
//! ```
//!
//! A missing file means the defaults. A broken file also means the defaults, and `dtk doctor`
//! says so: the hook must never fail because of a typo.

use crate::hook::config_dir;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub exclude_commands: Vec<String>,
    /// Replace a saved-to-file log with a digest of it.
    pub digest_logs: bool,
    /// Replace a web page that was fetched (`curl`) with its text.
    pub html_pages: bool,
    /// Also convert a whole HTML file that is read with the Read tool, when it is a page.
    pub html_reads: bool,
    pub min_tokens: usize,
    pub budget: usize,
    pub min_persisted_rows: usize,
    pub min_outline_tokens: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            exclude_commands: Vec::new(),
            digest_logs: true,
            html_pages: true,
            html_reads: true,
            min_tokens: 1200,
            budget: 2500,
            min_persisted_rows: 30,
            min_outline_tokens: 4000,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

/// The commented default file, as `dtk config --create` writes it.
pub fn default_file() -> String {
    let d = Config::default();
    format!(
        "# dtk configuration. Delete a line to get its default back.\n\n[hooks]\n# Leave these alone. \"^text\" matches the start of a command (a file path for Read, a tool name such as mcp__db__query for MCP);\n# any other text matches anywhere in it.\nexclude_commands = []\n# A log that Claude Code saved to a file (build and test output) becomes a digest of the whole.\ndigest_logs = true\n# A web page that was fetched (curl, wget) becomes its text; the page is saved for `dtk html`.\nhtml_pages = true\n# A big HTML page that is read with Read, or returned by an MCP tool, becomes its text too.\nhtml_reads = true\n\n[thresholds]\nmin_tokens = {}        # smallest output worth replacing (estimated tokens)\nbudget = {}            # largest summary\nmin_persisted_rows = {}  # fewest rows for output that Claude Code saved to a file\nmin_outline_tokens = {}  # smallest JSON document (not a table) worth an outline\n",
        d.min_tokens, d.budget, d.min_persisted_rows, d.min_outline_tokens
    )
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let t: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        let mut c = Config::default();
        if let Some(h) = t.get("hooks").and_then(|v| v.as_table()) {
            if let Some(a) = h.get("exclude_commands") {
                let arr = a
                    .as_array()
                    .ok_or("hooks.exclude_commands must be a list of strings")?;
                c.exclude_commands = arr
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .map(String::from)
                            .ok_or("hooks.exclude_commands must be a list of strings")
                    })
                    .collect::<Result<_, _>>()?;
            }
            if let Some(d) = h.get("html_reads") {
                c.html_reads = d
                    .as_bool()
                    .ok_or("hooks.html_reads must be true or false")?;
            }
            if let Some(d) = h.get("html_pages") {
                c.html_pages = d
                    .as_bool()
                    .ok_or("hooks.html_pages must be true or false")?;
            }
            if let Some(d) = h.get("digest_logs") {
                c.digest_logs = d
                    .as_bool()
                    .ok_or("hooks.digest_logs must be true or false")?;
            }
        }
        if let Some(th) = t.get("thresholds").and_then(|v| v.as_table()) {
            let num = |k: &str, into: &mut usize| -> Result<(), String> {
                if let Some(v) = th.get(k) {
                    let n = v
                        .as_integer()
                        .filter(|n| *n >= 0)
                        .ok_or(format!("thresholds.{k} must be a whole number"))?;
                    *into = n as usize;
                }
                Ok(())
            };
            num("min_tokens", &mut c.min_tokens)?;
            num("budget", &mut c.budget)?;
            num("min_persisted_rows", &mut c.min_persisted_rows)?;
            num("min_outline_tokens", &mut c.min_outline_tokens)?;
        }
        Ok(c)
    }

    /// The config from disk, or the defaults. The second value is why the file was not used.
    pub fn load() -> (Config, Option<String>) {
        let Some(p) = path() else {
            return (Config::default(), None);
        };
        match std::fs::read_to_string(&p) {
            Err(_) => (Config::default(), None),
            Ok(text) => match Config::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e)),
            },
        }
    }

    pub fn is_excluded(&self, subject: &str) -> bool {
        self.exclude_commands
            .iter()
            .any(|p| match p.strip_prefix('^') {
                Some(rest) => subject.trim_start().starts_with(rest),
                None => !p.is_empty() && subject.contains(p.as_str()),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn reads_values() {
        let c = Config::parse("[hooks]\nexclude_commands = [\"^kubectl\", \"psql\"]\n[thresholds]\nmin_tokens = 500\nbudget = 900\n").unwrap();
        assert_eq!(c.exclude_commands, ["^kubectl", "psql"]);
        assert_eq!(
            (c.min_tokens, c.budget, c.min_persisted_rows),
            (500, 900, 30)
        );
    }

    #[test]
    fn exclusion_rules() {
        let c = Config {
            exclude_commands: vec!["^kubectl".into(), "psql".into()],
            ..Config::default()
        };
        assert!(c.is_excluded("kubectl get pods"));
        assert!(c.is_excluded("  kubectl get pods"));
        assert!(!c.is_excluded("echo kubectl"), "^ means the start");
        assert!(
            c.is_excluded("PGPASSWORD=x psql -c 'select 1'"),
            "no ^ means anywhere"
        );
        assert!(!c.is_excluded("cat data.csv"));
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        assert!(Config::parse("[hooks\n").is_err());
        assert!(Config::parse("[thresholds]\nmin_tokens = \"many\"\n").is_err());
        assert!(Config::parse("[hooks]\nexclude_commands = [1]\n").is_err());
        assert!(Config::parse("[thresholds]\nbudget = -5\n").is_err());
    }

    #[test]
    fn the_default_file_parses_to_the_defaults() {
        assert_eq!(Config::parse(&default_file()).unwrap(), Config::default());
    }
}
