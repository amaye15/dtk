# dtk: data token killer

[![CI](https://github.com/amaye15/dtk/actions/workflows/ci.yml/badge.svg)](https://github.com/amaye15/dtk/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/dtk.svg)](https://crates.io/crates/dtk)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)

**Big tables, logs and JSON in command output cost tokens and bury the answer. `dtk` swaps them for a short summary of every row. The full data stays on disk.**

A [Claude Code](https://claude.com/claude-code) hook. One Rust binary (about 1.1 MB, no runtime dependencies) that works with [rtk](https://github.com/rtk-ai/rtk).

## Example

A CI job failed. Its log has 1,169 lines (about 21,700 tokens). Claude Code would show the model the first 550 tokens, all runner set-up text. With `dtk` the model sees 567 tokens, which end with:

```text
[dtk] text-log: 1169 rows x 3 cols, 84.8KB (statistics over all rows; the full data is on disk)
...
1 error, 2 warnings (flag).
showing 7 lines, each error with the 4 lines before it:
2026-11-02T02:25:03.5979589Z,(Reading database ... 212983 files and directories currentl…,
2026-11-02T02:25:03.6020420Z,Removing dtk (0.3.0-1) ...,
2026-11-02T02:25:03.6214925Z,/usr/bin/dtk,
2026-11-02T02:25:03.6229783Z,##[error]Process completed with exit code 1.,error
```

The cause is in view: the package was removed, and `/usr/bin/dtk` was still there.

## Does it hurt the answers?

`scripts/ab-test.sh` asks real Claude Code twenty-four questions about data files, with and without dtk. They include a column with `N/A` in it, prices written as `$1,234`, a filter on two columns, a table with 60 feature columns, a Python list of records, a pandas JSON file, the mean per group, and the sampling interval of a sensor log. The true answers come from `jaq`, `qsv`, `awk` and `grep`, not from dtk.

- **Checked again at 0.13.1 (all 24 questions, Haiku, one run each):** 24 of 24 right with dtk and 24 of 24 without; tokens from tool results fell 72% to 89% on 20 of them. The four with a small saving are the cases below where Claude Code had already cut the output. Added in 0.15.0: `scripts/tools-test.sh`, four questions about a test log, a compiler log and a web page through stand-in programs (Sonnet right every time, 2 turns instead of 3 or 4 on the logs, 92% fewer tokens on the page). Added in 0.14.0: a script run with `uv run` whose output rtk cuts to fifty rows (Sonnet and Haiku right 2 of 2 with and without dtk; 4 turns instead of 6 and a third fewer tokens with dtk), and `scripts/gh-test.sh`, two questions about `gh` output through a stand-in `gh` (right every time, up to 85% fewer tokens). `scripts/mcp-test.sh` asks four more through a real MCP server (Sonnet and Haiku, right every time with dtk, 93% to 94% fewer tokens on tables).
- **Answers: no loss.** Sonnet: 34 of 34 right with dtk, 34 of 34 without. Haiku: 34 of 34 and 34 of 34. (17 questions, 2 runs each.) Three questions added in 0.11.0 (pandas `groupby` output with a MultiIndex, with one and with two lines of column names, and the row with the smallest of tiny numbers): Sonnet and Haiku 2 of 2 right with and without dtk, 78% to 80% fewer tokens. Two more in 0.12.0 (`print(df.to_dict())`, and a frame with a datetime column), same result, 78% and 81% fewer.
- **Tokens from tool results fell 69% to 90% on thirteen of the seventeen questions** (300-row CSV 4,549 → 498, `ps aux` 2,072 → 623, a column with `N/A` 5,456 → 556). Turns fell from 6 to 4 on six questions.
- **Where the saving is small,** Claude Code had already cut the output to a preview of about 550 tokens (a big CI log, a wide table, a big JSON file). The count stays about the same, and the model gets a summary of every row in place of the preview.

## Quick start

```sh
curl -fsSL https://raw.githubusercontent.com/amaye15/dtk/main/install.sh | sh
```

That is all: the installer registers the hook in `~/.claude/settings.json` (with a backup) and chains it to rtk when rtk is installed. Set `DTK_NO_INIT=1` to install without registering. With Homebrew, Scoop, Cargo or a package, register once with `dtk init --global`; `dtk verify` checks detection, the hook and rtk.

Start a new Claude Code session (hooks load at start). To see what dtk would do first, run `dtk discover` (reads your history, changes nothing) or `dtk mode shadow`.

| System | Install |
|---|---|
| macOS, Linux | `curl -fsSL https://raw.githubusercontent.com/amaye15/dtk/main/install.sh \| sh` |
| Windows | `irm https://raw.githubusercontent.com/amaye15/dtk/main/install.ps1 \| iex` |
| Homebrew, Scoop | `brew install amaye15/tap/dtk`, or `scoop bucket add amaye15 https://github.com/amaye15/scoop-bucket` then `scoop install dtk` |
| Cargo | `cargo install dtk --locked` |
| winget | [pull request](https://github.com/microsoft/winget-pkgs/pull/468535) pending; meanwhile use [`packaging/winget`](packaging/winget) |
| `.deb`, `.rpm`, archives | the [releases page](https://github.com/amaye15/dtk/releases) (macOS, Linux, Windows; each file has a `.sha256`) |

[docs/INSTALL.md](docs/INSTALL.md) has update and uninstall steps. Another program named `dtk` exists; `dtk verify` printing `built-in checks passed` shows you have this one.

## Automatic, and nothing to call

dtk has no commands to learn for its main job. After `dtk init --global`, every big table, log or JSON document in a command's output is replaced by its summary, by the hook, on its own, chained to rtk. The summary is made to hold the answers to the usual questions, so that the model does not need a second step: the ranges, the counts, the row of each extreme, the mean of a measure per group (`amount by region (mean): ap 166.8, us 161.8, eu 101.5`), the outliers, the time gaps, the correlated columns.

When a question is not answered there, the summary ends with a `drill:` line. These commands are a fallback, not the interface. The model runs them through the Bash tool, and rtk does not rewrite them or cut their output:

```text
drill: dtk where 5fa6aeae75bd region = eu  |  dtk top 5fa6aeae75bd amount 10  |  dtk group 5fa6aeae75bd region amount  |  (also rows, count, stats, get: dtk rows --help)
```

`5fa6aeae75bd` is the short id of the data file. dtk needs no other program. These extras are optional:

| Tool | Gives you | Without it |
|---|---|---|
| `qsv` | SQL on the data files (`qsv sqlp`) | nothing is lost for the common questions |
| `jaq` | jq programs on JSON | `dtk get` reads a path |
| `rtk` | the chain, and rtk's totals in `dtk gain` | dtk works alone |

`brew install qsv jaq` if you want them.

## What it recognizes

dtk reads Bash output, whole-file `Read` of data files, and the results of MCP tools (database rows, issue lists). Detection is strict: when dtk is not sure, the output passes through untouched.

- **Rows and columns:** JSON Lines, JSON arrays, objects that wrap one big array, CSV/TSV (`;` and `|` too, and pandas `to_csv()` with its index), bordered tables (polars, psql, MySQL, SQLite, Markdown, rich), aligned tables (pandas `to_string()`, with datetime columns too, and `groupby().agg()`, `pivot_table` and Series output with a MultiIndex), `ps aux`, `ps -ef`, `df -h`, logfmt, access logs, `docker compose logs`, and plain text logs whose lines start with a timestamp (CI, application, syslog).
- **Data as other tools print it:** a Python list of dicts (`print(df.to_dict("records"))`), the other ways pandas prints a frame (`to_dict()`, `to_dict("index")`, `to_dict("split")`, `to_json()`, `values.tolist()`, one dict per line), pandas `to_json(orient="split")`, a printed number matrix, delimited text with no header (`psql -t -A`, `sqlite3 -list`), one-block-per-row output (`psql -x`, `sqlite3 -line`, MySQL `\G`), and a page that is one HTML table (`df.to_html()`).
- **With text around it:** up to six short lines of text before or after the data stay as they are, and only the data is replaced: a status line above a JSON array, a title above a CSV, a total below it, or a database server's warning on both sides of its rows. Progress lines of `uv` and `pip` (`Downloading ...`, `Installed 13 packages`) come before a script's output when stderr is merged (`uv run script.py 2>&1`); they do not count against the six.
- **GitHub CLI output:** `gh run list`, `gh pr list`, `gh issue list`, `gh release list`, `gh repo list`, `gh search ...`, `gh pr checks` and `gh label list` print tab-separated lines with no header when they are not on a terminal; the command names the columns (`status`, `conclusion`, `title`, `workflow`, ...). `gh run view --log` and `--log-failed` become a log with the `job` and `step` of each line, with the errors shown. Not when `--json`, `--jq` or `--template` shaped the output.
- **A long log:** build and test output that Claude Code saved to a file (`cargo test`, `pytest -v`, a compiler's errors, an install log): a digest of all of it, built by counting, with no model. It says what the log is made of, lists every kind of error and warning with the line number of the first and the lines under it, how many there are and which files they are in, and the last lines, where test runners say how it ended. Claude Code shows the model only the first lines of such a file, and they are almost always the boring ones. It is for logs only: `cat`, `grep`, `find`, `git diff` and `curl` print content, and source code, documents and lists are never digested.
- **A web page** that was fetched to be read (`curl`, `wget`, a script with an address in it), returned by an MCP tool (a fetch or browser server), or a big HTML file read with `Read`: its visible text, with headings, lists, tables and links, and without the markup, scripts and styles (a page is 85% to 90% markup). The page is saved, and `dtk html ID text|headings|links|tables|json|grep` reads any part of it. `cat page.html` is a person working on markup, and is never converted. Neither is a `Read` with `offset` or `limit`, a file under about 10 KB, one with template syntax (`{{`, `{%`, `<?php`, Vue or Angular directives), or the result of an MCP tool that reads files from disk. A file that was read says how to see its markup.
- **An outline:** a JSON document that is not a table (`cargo metadata`, `terraform show -json`). It lists each key with its size, the merged shape of the records in each array, and a `jaq` command to read any part.

Up to 60 rows become CSV and nothing is dropped. A bigger table gets:

- shape, and every column name with its type (a run such as `feat_0..feat_119:float` is one range). Nested JSON objects are opened into columns of their own (`owner.login`, `head_commit.author.name`), so each field has statistics and none is a blob of JSON text.
- per column that varies: min, max, mean, median and p95; value counts; time range; nulls
- one line each, and no statistics line, for columns that say nothing more: those that are **the same in every row** (`version=v1`), **different in every row** (ids, URLs, hashes), or **group the rows exactly like an earlier column** (`same groups as actor.login: actor.id, actor.url, ...`). Every column is still named, so nothing is lost.
- the mean of each measure per group, for the splits that matter most (the leftmost label column against the leftmost measure always; others when the groups explain a quarter of the variance): `amount by region (mean): ap 166.8, us 161.8, eu 101.5`
- numbers and dates as people write them: `$1,234.56`, `45.3%` and `1,234` are numbers, `03/21/2025` is a date when the data settles the order, and `N/A`, `-` or `?` in a column of numbers are counted and set aside, not mistaken for words
- data-quality notes: outliers beyond 3×IQR, rows that repeat an earlier row, and columns that move together (|r| of 0.9 or more)
- for a time column: whether the rows are in time order, the step between them (`every 5min`), and the longest gap (`3h5min after row 148`)
- **the row that holds each numeric column's largest and smallest value** (`max at order_id=100243`)
- a few first and last rows, and the **full rows of the extremes** (`extremes (largest amount; smallest amount; largest latency_ms): ...`), so that "who had the largest amount?" needs no second step. Message shapes come with counts (`request # took #ms (2000)`).
- the rows where `level` says warn or error. In a text log, each error comes with the 4 lines before it, and repeated errors are grouped. A log with no level field gets a `flag` column that dtk derives from words such as `error:` and `FAILED`, and the summary says so.
- the `dtk` commands to read more of the data

**It never touches:**

- source code, prose, documents with a table in them, and directory listings (`ls -l`, `find`): the exact text or the names are the content
- small outputs: under 1,200 estimated tokens for a table, 4,000 for a JSON document (8,000 for `Read`)
- requests for rows: `head`, `tail`, `sed -n`, `qsv slice`, rtk's `--head-lines` and `--tail-lines`, `jq`/`jaq` queries, dtk's own drill-down commands, reads with `offset` or `limit`, and any file dtk saved itself
- output that rtk cut when rtk's saved copy of it cannot be read (`... (N lines truncated)`): a summary would claim rows that are missing. When the copy can be read, dtk summarizes that (see below)
- a JSON document that an MCP tool returns: the model asked a server for those values, so only tables are summarized there
- commands listed in `exclude_commands`; for an MCP tool the name is the tool name (`^mcp__github__`)

## Works with rtk

`dtk init --global` registers **one** `PostToolUse` hook: `dtk hook claude --then rtk hook claude`. dtk looks at each event first. If the output is data, dtk answers. Anything else goes to rtk unchanged. dtk goes first because a table that rtk had already cut would be summarized wrongly.

- **`rtk init` keeps working.** dtk's entry holds a silent placeholder named `rtk`, so `rtk init -g` adds no second hook that could replace dtk's summary.
- **dtk reads what rtk cut.** rtk rewrites `uv run python script.py` to `rtk uv run ...` and cuts that output to about fifty lines with a pointer to the rest, so a script that prints 800 rows reaches the model as 50. rtk keeps the whole output, and dtk reads it (the tee file in the pointer, or `rtk recall --full` for `[+N hidden: rtk recall HASH]`) and summarizes all of it: a 500-row DataFrame printed through `uv run` went from 50 rows in 919 tokens to a summary of all 500 rows in 452. dtk uses only a complete copy that sits in rtk's own `rtk/tee` folder and starts as the cut output does; a copy that rtk itself truncated (it keeps the first megabyte) is not used.
- **dtk sees through rtk's rewrites.** rtk turns `head -n 400 f` into `rtk read f --head-lines 400`. dtk still treats it as a request for 400 rows, and `exclude_commands = ["^cat "]` still matches `rtk read f`.
- **rtk's own `PostToolUse` entry is taken over and put back** by `dtk init --uninstall`. rtk's `PreToolUse` hook is never touched.
- **dtk's own commands are never touched by rtk.** rtk's pre-command hook does not rewrite `dtk top ...`, and dtk keeps these results out of rtk's post-command pipeline, which would cut a long one.
- The chain adds a few milliseconds (dtk 2 to 7 ms, rtk 4 to 13 ms, both 17 to 21 ms). `dtk init --global --no-rtk` skips it.

All of this is tested against the real rtk binary (`tests/rtk_real.rs`, `tests/rtk_compat.rs`, and `tests/rtk_windows.rs`).

## Commands

| Command | Effect |
|---|---|
| `dtk init --global [--no-rtk]`, `dtk init --uninstall [--purge]` | Register or remove the hook. `--purge` also deletes dtk's data and config. |
| `dtk mode shadow`, `dtk mode live` | Shadow decides and logs but changes no output. Live replaces output (the default). |
| `dtk doctor`, `dtk verify` | Check the setup. `verify` also runs built-in checks of every format. They exit 1 on a warning. |
| `dtk explain FILE --command CMD` | Say whether dtk would summarize this output, and why. |
| `dtk discover [--since DAYS]` | Scan your Claude Code history for output dtk would have replaced. Changes nothing. |
| `dtk rows`, `top`, `count`, `group`, `where`, `stats`, `get` | Read more of the data behind a summary: rows, the largest values, value counts, mean/sum/min/max per group, a filter, column statistics, a part of a JSON document. `dtk rows --help` has the syntax. |
| `dtk html ID WHAT` | Read more of a web page that dtk saved: `text [FROM [COUNT]]`, `headings`, `links [N]`, `tables [N]`, `json [N]` (embedded JSON), `grep WORD`. |
| `dtk view FILE`, `dtk gain [--json]`, `dtk config [--create]` | Show a summary, the decision log totals (with rtk's), or the settings. |

`DTK_DISABLED=1` passes every output through. `DTK_RAW=1 cat data.csv` passes one command through.

## Config

`~/.config/dtk/config.toml` (`dtk config --create` writes it). A broken file means the defaults, and `dtk doctor` reports it.

```toml
[hooks]
exclude_commands = ["^kubectl", "psql"]   # "^x" matches the start of a command; "x" matches anywhere
digest_logs = true                        # a saved build or test log becomes a digest
html_pages = true                         # a fetched web page becomes its text
html_reads = true                         # so does a big HTML file read with Read, or returned by an MCP tool

[thresholds]
min_tokens = 1200          # smallest table worth replacing (estimated tokens)
budget = 2500              # largest summary
min_persisted_rows = 30    # fewest rows for output that Claude Code saved to a file
min_outline_tokens = 4000  # smallest JSON document worth an outline
```

## Safety

- **The hook fails open.** On any error dtk adds nothing, and Claude Code keeps the original output. A chained command that is missing or takes over 10 seconds is skipped.
- dtk reads only files under `~/.claude/`. It does not touch MCP tool output.
- A summary replaces the output only if it is at least 30% smaller, or if Claude Code saved the output to a file.
- Full copies of replaced data are in `~/.local/share/dtk/store/` (mode 0600; newest 60 files, nothing older than 14 days). The decision log is `decisions.jsonl` next to it. Command output can contain secrets, so treat the store like the files Claude Code saves in `~/.claude/`.

## How it is checked

252 tests, plus the A/B script and an end-to-end test with a real MCP server. Real output of pandas, numpy, polars, sqlite3, rich, jq, git, `gh`, `ps` and CI logs, and messy and wide data sets (79 files detected as expected, 36 rejected, 7 found inside text, 9 `gh` outputs read by their command, 3 real logs and a real web page; no source file, document or negative fixture of this repository is ever taken for a log). A test that lists the facts a summary must keep and checks them (2123 of 2123 kept). dtk's statistics and its drill-down commands compared with `qsv`, `awk` and plain sorting. Fuzzing. The real rtk binary. CI on macOS, Linux and Windows, and a test of every install route against each release. Details: [docs/PLAN-0.5.0.md](docs/PLAN-0.5.0.md).

## Limits

- Aligned text is a heuristic, and a wrong guess puts values under the wrong name. So dtk refuses what it cannot read safely: pandas `print(df)` (columns hidden behind `...`), `to_string()` with a DatetimeIndex or with a datetime in the first column and no index (the index cannot be told from the value), and tables whose column names contain spaces (`netstat`, `lsof`). A MultiIndex is read when pandas prints the line of index names, which it does for named levels (`groupby` always has them). Print data with `df.to_csv()` or `df.to_json(orient="records", lines=True)`.
- Inputs above 128 MB are skipped.
- dtk is a separate program that calls rtk, not an rtk plugin: a plugin needs a custom rtk build and would see only the first 30,000 characters of a large output.
- Claude Code only. Windows has an x86_64 build and no ARM64 build.

## Develop

```sh
cargo test && cargo clippy --all-targets -- -D warnings
scripts/ab-test.sh     # real Claude Code, with and without dtk
```

As a library: `dtk::summarize(&text, &dtk::lean::Ctx { bytes: 0, csv_path: None, raw_path: None }, 2500)` returns the table and its summary.

MIT. See [LICENSE-MIT](LICENSE-MIT).
