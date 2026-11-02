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

`scripts/ab-test.sh` asks real Claude Code fifteen questions about data files, with and without dtk. They include a column with `N/A` in it, prices written as `$1,234`, a filter on two columns, a table with 60 feature columns, a Python list of records and a pandas JSON file. The true answers come from `jaq`, `qsv`, `awk` and `grep`, not from dtk.

- **Answers: no loss.** Sonnet: 32 of 32 right with dtk, 32 of 32 without. Haiku: 45 of 45 and 45 of 45. (2 to 3 runs per question and model.)
- **Tokens from tool results fell 69% to 89% on eleven of the fifteen questions** (300-row CSV 4,549 → 498, `ps aux` 2,072 → 623, a column with `N/A` 5,456 → 556). Turns fell from 6 to 4 on six questions.
- **Where the saving is small,** Claude Code had already cut the output to a preview of about 550 tokens (a big CI log, a wide table, a big JSON file). The count stays about the same, and the model gets a summary of every row in place of the preview.

## Quick start

```sh
brew install amaye15/tap/dtk     # or any way below
dtk init --global                # registers the hook; writes a backup of settings.json first
dtk verify                       # checks detection, the hook and rtk
```

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

## Optional tools

dtk needs nothing else. A summary ends with a `drill:` line of dtk's own commands, so the model can read more of the data with no other program:

```text
drill: dtk count 5fa6aeae75bd region  |  dtk top 5fa6aeae75bd amount 10  |  dtk where 5fa6aeae75bd region = eu  |  dtk rows 5fa6aeae75bd 100 20
```

`5fa6aeae75bd` is the short id of the data file. These extras are optional:

| Tool | Gives you | Without it |
|---|---|---|
| `qsv` | SQL on the data files (`qsv sqlp`) | nothing is lost; `dtk top`, `count`, `where`, `rows` and `stats` cover the common questions |
| `jaq` | jq programs on JSON | the same; `dtk get` reads a path |
| `rtk` | the chain, and rtk's totals in `dtk gain` | dtk works alone |

`brew install qsv jaq` if you want them.

## What it recognizes

dtk reads Bash output and whole-file `Read` of data files. Detection is strict: when dtk is not sure, the output passes through untouched.

- **Rows and columns:** JSON Lines, JSON arrays, objects that wrap one big array, CSV/TSV (`;` and `|` too, and pandas `to_csv()` with its index), bordered tables (polars, psql, MySQL, SQLite, Markdown, rich), aligned tables (pandas `to_string()`), `ps aux`, `ps -ef`, `df -h`, logfmt, access logs, `docker compose logs`, and plain text logs whose lines start with a timestamp (CI, application, syslog).
- **Data as other tools print it:** a Python list of dicts (`print(df.to_dict("records"))`), pandas `to_json(orient="split")`, a printed number matrix, delimited text with no header (`psql -t -A`, `sqlite3 -list`), one-block-per-row output (`psql -x`, `sqlite3 -line`, MySQL `\G`), and a page that is one HTML table (`df.to_html()`).
- **An outline:** a JSON document that is not a table (`cargo metadata`, `terraform show -json`). It lists each key with its size, the merged shape of the records in each array, and a `jaq` command to read any part.

Up to 60 rows become CSV and nothing is dropped. A bigger table gets:

- shape, and every column name with its type (a run such as `feat_0..feat_119:float` is one range)
- per column: min, max, mean, median and p95; value counts; time range; nulls
- numbers and dates as people write them: `$1,234.56`, `45.3%` and `1,234` are numbers, `03/21/2025` is a date when the data settles the order, and `N/A`, `-` or `?` in a column of numbers are counted and set aside, not mistaken for words
- data-quality notes: outliers beyond 3×IQR, and rows that repeat an earlier row
- **the row that holds each numeric column's largest and smallest value** (`max at order_id=100243`)
- the first and last rows, and message shapes with counts (`request # took #ms (2000)`)
- the rows where `level` says warn or error. In a text log, each error comes with the 4 lines before it, and repeated errors are grouped. A log with no level field gets a `flag` column that dtk derives from words such as `error:` and `FAILED`, and the summary says so.
- the `dtk` commands to read more of the data

**It never touches:**

- source code, prose, and directory listings (`ls -l`, `find`): the exact text or the names are the content
- small outputs: under 1,200 estimated tokens for a table, 4,000 for a JSON document (8,000 for `Read`)
- requests for rows: `head`, `tail`, `sed -n`, `qsv slice`, rtk's `--head-lines` and `--tail-lines`, `jq`/`jaq` queries, dtk's own drill-down commands, reads with `offset` or `limit`, and any file dtk saved itself
- output that rtk already cut (`... (N lines truncated)`): a summary would claim rows that are missing
- commands listed in `exclude_commands`

## Works with rtk

`dtk init --global` registers **one** `PostToolUse` hook: `dtk hook claude --then rtk hook claude`. dtk looks at each event first. If the output is data, dtk answers. Anything else goes to rtk unchanged. dtk goes first because a table that rtk had already cut would be summarized wrongly.

- **`rtk init` keeps working.** dtk's entry holds a silent placeholder named `rtk`, so `rtk init -g` adds no second hook that could replace dtk's summary.
- **dtk sees through rtk's rewrites.** rtk turns `head -n 400 f` into `rtk read f --head-lines 400`. dtk still treats it as a request for 400 rows, and `exclude_commands = ["^cat "]` still matches `rtk read f`.
- **rtk's own `PostToolUse` entry is taken over and put back** by `dtk init --uninstall`. rtk's `PreToolUse` hook is never touched.
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
| `dtk rows`, `top`, `count`, `where`, `stats`, `get` | Read more of the data behind a summary: rows, the largest values, value counts, a filter, column statistics, a part of a JSON document. `dtk rows --help` has the syntax. |
| `dtk view FILE`, `dtk gain [--json]`, `dtk config [--create]` | Show a summary, the decision log totals (with rtk's), or the settings. |

`DTK_DISABLED=1` passes every output through. `DTK_RAW=1 cat data.csv` passes one command through.

## Config

`~/.config/dtk/config.toml` (`dtk config --create` writes it). A broken file means the defaults, and `dtk doctor` reports it.

```toml
[hooks]
exclude_commands = ["^kubectl", "psql"]   # "^x" matches the start of a command; "x" matches anywhere

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

180 tests, plus the A/B script. Real output of pandas, numpy, polars, sqlite3, rich, jq, git, `gh`, `ps` and CI logs, and messy and wide data sets (62 files detected as expected, 30 rejected). A test that lists the facts a summary must keep and checks them (1148 of 1148 kept). dtk's statistics and its drill-down commands compared with `qsv`, `awk` and plain sorting. Fuzzing. The real rtk binary. CI on macOS, Linux and Windows, and a test of every install route against each release. Details: [docs/PLAN-0.5.0.md](docs/PLAN-0.5.0.md).

## Limits

- Aligned text is a heuristic, and a wrong guess puts values under the wrong name. So dtk refuses what it cannot read safely: pandas `print(df)` (columns hidden behind `...`), `to_string()` with a datetime column, and tables whose column names contain spaces (`netstat`, `lsof`). Print data with `df.to_csv()` or `df.to_json(orient="records", lines=True)`.
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
