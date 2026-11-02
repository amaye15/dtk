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

`scripts/ab-test.sh` asks real Claude Code ten questions about data files, with and without dtk. The true answers come from `jaq`, `qsv`, `awk` and `grep`. Sonnet, 2 runs per case:

- Tokens from tool results fell 70% to 90% on seven cases (300-row CSV 4,550 → 445, `ps aux` 2,072 → 609, `docker compose logs` 4,498 → 449). Turns fell from 6 to 4 on five.
- **19 of 20 answers right with dtk, 20 of 20 without.** The one miss did not repeat in 5 more runs.
- Claude Code already cuts output over 30,000 characters to a preview of about 550 tokens. Where that applies (a big CI log or JSON file), the token count stays about the same, and the model gets a summary of every row in place of the preview.
- Haiku sometimes writes about the tool and does not state the answer. Sonnet did not in 6 of 6 runs.

## Quick start

```sh
brew install amaye15/tap/dtk     # or any way below
dtk init --global                # registers the hook; writes a backup of settings.json first
dtk verify                       # checks detection, the hook, rtk, qsv and jaq
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

dtk runs alone. These tools make it better:

| Tool | Used for | Without it |
|---|---|---|
| `qsv` | the `drill:` commands in table summaries, which the model runs to read other rows | summaries work; the model cannot run those commands; `dtk doctor` warns |
| `jaq` | the same, for JSON | the same |
| `rtk` | the chain, and rtk's totals in `dtk gain` | dtk works alone |

Install the first two with `brew install qsv jaq`. dtk makes every summary itself and never runs `qsv` or `jaq`. Only the model does, in a later step.

## What it recognizes

dtk reads Bash output and whole-file `Read` of data files. Detection is strict: when dtk is not sure, the output passes through untouched.

- **Rows and columns:** JSON Lines, JSON arrays, objects that wrap one big array, CSV/TSV (`;` and `|` too), bordered tables (polars, psql, MySQL, SQLite, Markdown, rich), aligned tables (pandas `to_string()`), `ps aux`, `ps -ef`, `df -h`, logfmt, access logs, `docker compose logs`, and plain text logs whose lines start with a timestamp (CI, application, syslog).
- **An outline:** a JSON document that is not a table (`cargo metadata`, `terraform show -json`). It lists each key with its size, the merged shape of the records in each array, and a `jaq` command to read any part.

Up to 60 rows become CSV and nothing is dropped. A bigger table gets:

- shape, column names and types
- per column: min, max, mean, median and p95; value counts; time range; nulls
- **the row that holds each numeric column's largest and smallest value** (`max at order_id=100243`)
- the first and last rows, and message shapes with counts (`request # took #ms (2000)`)
- the rows where `level` says warn or error. In a text log, each error comes with the 4 lines before it, and repeated errors are grouped. A log with no level field gets a `flag` column that dtk derives from words such as `error:` and `FAILED`, and the summary says so.
- the commands to query the full data

**It never touches:**

- source code, prose, and directory listings (`ls -l`, `find`): the exact text or the names are the content
- small outputs: under 1,200 estimated tokens for a table, 4,000 for a JSON document (8,000 for `Read`)
- requests for rows: `head`, `tail`, `sed -n`, `qsv slice`, rtk's `--head-lines` and `--tail-lines`, `jq`/`jaq` queries, reads with `offset` or `limit`, and any file dtk saved itself
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

144 tests, plus the A/B script. Real output of pandas, polars, sqlite3, rich, jq, git, `gh`, `ps` and CI logs (50 files detected as expected, 27 rejected). A test that lists the facts a summary must keep and checks them (759 of 759 kept). dtk's statistics compared with `qsv`, `jaq` and `awk`. Fuzzing. The real rtk binary. CI on macOS, Linux and Windows, and a test of every install route against each release. Details: [docs/PLAN-0.5.0.md](docs/PLAN-0.5.0.md).

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
