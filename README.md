# dtk: data token killer

[![CI](https://github.com/amaye15/dtk/actions/workflows/ci.yml/badge.svg)](https://github.com/amaye15/dtk/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/dtk.svg)](https://crates.io/crates/dtk)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)

**Big tables, logs and JSON in command output cost tokens and bury the answer. `dtk` swaps them for a short summary that covers every row, and keeps the full data on disk.**

It is a [Claude Code](https://claude.com/claude-code) hook. It is one Rust binary (about 1.1 MB, no runtime dependencies) and it works together with [rtk](https://github.com/rtk-ai/rtk).

## See it work

A CI job failed. The log has 1,169 lines, about 21,700 tokens. Claude Code would show the model a preview of the first 550 tokens, and all of those are runner set-up text. With `dtk`, the model sees this (567 tokens):

```text
[dtk] text-log: 1169 rows x 3 cols, 84.8KB (statistics over all rows; the full data is on disk)
data: ~/.local/share/dtk/store/c49b697429e5.csv (csv, header row)
cols: ts:time msg:str flag:str
ts: 2026-11-02T02:21:55.2959628Z..2026-11-02T02:25:03.9167443Z
...
1 error, 2 warnings (flag).
showing 7 lines, each error with the 4 lines before it:
2026-11-02T02:22:48.4966802Z,warning: package `spin v0.9.8` in Cargo.lock is yanked in r…,warn
2026-11-02T02:25:03.5035021Z,20 of 20 built-in checks passed,
2026-11-02T02:25:03.5979589Z,(Reading database ... 212983 files and directories currentl…,
2026-11-02T02:25:03.6020420Z,Removing dtk (0.3.0-1) ...,
2026-11-02T02:25:03.6214925Z,/usr/bin/dtk,
2026-11-02T02:25:03.6229783Z,##[error]Process completed with exit code 1.,error
```

The cause is in view: the package was removed, and `/usr/bin/dtk` was still there. (Paths are shortened with `~`, and some lines are left out.) The same hook handles a 300-row CSV, and the summary says which row holds each extreme:

```text
[dtk] csv: 300 rows x 7 cols, 17.7KB (statistics over all rows; the full data is on disk)
data: ~/.local/share/dtk/store/5fa6aeae75bd.csv (csv, header row)
cols: order_id:int ts:time customer:str region:str amount:float status:str latency_ms:int
region: 4 distinct: ap-south 80, us-west 80, us-east 77, eu-central 63
amount: 1.78..369.19 mean 83.4017 p50 67.91 p95 199.62 | max at order_id=100243, min at order_id=100136
status: 3 distinct: ok 179, retry 71, failed 50
latency_ms: 9..896 mean 448.2 p50 419 p95 858 | max at order_id=100104, min at order_id=100184
head 8 (columns in order above):  ...
tail 4:  ...
drill: qsv sqlp ~/.local/share/dtk/store/5fa6aeae75bd.csv 'select "region", count(*) n from _t_1 group by "region" order by n desc'
```

## Does it hurt the answers?

I ran this test to find out. `scripts/ab-test.sh` asks real Claude Code ten questions about data files, once with `dtk` and once without. The true answers come from `jaq`, `qsv`, `awk` and `grep`, not from dtk. Sonnet, average of 2 runs per cell:

| Question about | Tokens from tool results, without → with | Turns |
|---|---:|---:|
| 300-row CSV: which region has the most orders | 4,550 → 445 | 6 → 4 |
| 300-row CSV: which order had the highest latency | 4,536 → 445 | 4 → 4 |
| 300 JSON lines | 568 → 505 | 6 → 4 |
| logfmt service log | 3,167 → 750 | 4 → 4 |
| web access log | 3,822 → 721 | 4 → 4 |
| Kubernetes pod list (JSON) | 1,735 → 251 | 4 → 4 |
| `docker compose logs` | 4,498 → 449 | 6 → 4 |
| `ps aux` | 2,072 → 609 | 4 → 4 |
| CI log (failed job) | 603 → 563 | 6 → 4 |
| `cargo metadata` (JSON document) | 573 → 304 | 3 → 2 |

The last row is the average of 5 re-runs.

- **Answers:** 19 of 20 right with dtk, and 20 of 20 without. The one miss did not repeat in 5 more runs.
- **Where the saving is small:** Claude Code already cuts any output over 30,000 characters to a preview of about 550 tokens. `dtk` swaps that preview for a summary of about the same size that covers every row, so the model knows more and the cost is the same.
- **Haiku is noisier.** It sometimes writes about the tool and does not state the answer. Sonnet did not do that in 6 of 6 runs.

Run it yourself: `scripts/ab-test.sh` (it registers the hooks in a throwaway project and does not touch your settings).

## Quick start

```sh
brew install amaye15/tap/dtk      # or any way in the table below
dtk init --global                 # registers the hook (writes a backup of settings.json first)
dtk verify                        # checks detection, the hook, rtk, qsv and jaq
```

Start a new Claude Code session. A running session does not change, because hooks load at start. To see what dtk would do before it changes anything, run `dtk discover` (it reads your history and changes nothing) or `dtk mode shadow`.

| System | Command |
|---|---|
| macOS, Linux | `curl -fsSL https://raw.githubusercontent.com/amaye15/dtk/main/install.sh \| sh` (checks the SHA-256) |
| Homebrew | `brew install amaye15/tap/dtk` |
| Windows (PowerShell) | `irm https://raw.githubusercontent.com/amaye15/dtk/main/install.ps1 \| iex` |
| Scoop | `scoop bucket add amaye15 https://github.com/amaye15/scoop-bucket` then `scoop install dtk` |
| winget | submitted to the community repository ([pull request](https://github.com/microsoft/winget-pkgs/pull/468535)); until it merges, use the manifests in [`packaging/winget`](packaging/winget) |
| Cargo | `cargo install dtk --locked` |
| Debian, Ubuntu | `dtk_amd64.deb` or `dtk_arm64.deb` from the [releases page](https://github.com/amaye15/dtk/releases) |
| Fedora, RHEL | `dtk.x86_64.rpm` or `dtk.aarch64.rpm` from the releases page |
| Any | static archives for macOS, Linux and Windows, each with a `.sha256` file |

[docs/INSTALL.md](docs/INSTALL.md) has update and uninstall steps for each way. Another program named `dtk` exists (decomp-toolkit). If `dtk verify` prints `built-in checks passed`, you have this one. For the drill-down commands in the summaries, install `qsv` and `jaq` (`brew install qsv jaq`).

## What it recognizes

`dtk` looks at Bash output and at whole-file `Read` of data files. Detection is strict: when it is not sure, the output passes through untouched.

| Found in the output | Shown as |
|---|---|
| JSON Lines, JSON array of objects | rows and columns |
| A JSON object that wraps one big array (`{"items": [...]}`), or is keyed by id | rows and columns |
| CSV, TSV, `;` or `\|` separated text with a header | rows and columns |
| Tables with borders: polars, psql, MySQL, SQLite, Markdown, rich | rows and columns |
| Whitespace-aligned tables, such as pandas `to_string()` | rows and columns |
| `ps aux`, `ps -ef`, `df -h` (the last column is free text) | rows and columns |
| logfmt (`level=info msg="x"`) | rows and columns |
| Apache and nginx access logs | rows and columns |
| JSON lines behind a prefix (`docker compose logs`, `kubectl logs --prefix`) | rows and columns |
| Plain text logs whose lines start with a timestamp: CI logs, application logs, syslog. A stack trace belongs to the line above it. | rows and columns, with errors and their context |
| A JSON document that is not a table: `cargo metadata`, `terraform show -json`, a few big API records | an outline |

A table of 60 rows or fewer is re-encoded as CSV and nothing is dropped. A bigger table gets:

- the shape and size, the column names and types
- per column: min, max, mean, median and p95 for numbers; value counts for labels and codes; the time range; null counts
- for each numeric column, **the row that holds its largest and smallest value** (`max at order_id=100243`), so "which order was the slowest?" needs no second command
- for text with many different values, the few message shapes with counts (`request # took #ms (2000)`)
- the first rows and the last rows
- the rows where a `level` says warn or error. In a text log, each error comes with the 4 lines before it, repeated errors are grouped by shape, and a log without a level field gets a `flag` column that dtk derives from words that lead a message or label it (`error:`, `FAILED`, `##[error]`). The summary says so.
- the exact commands to query the full data with `qsv` and `jaq`

A **JSON document** gets an outline instead: each top-level key with its size, the merged shape of the records in each array (a key that only some records have is marked `?`, a field that is sometimes null reads `str|null`), and a `jaq` command to read any part.

### What it never touches

- Source code and prose. The model needs the exact text.
- Directory listings (`ls -l`, `find`, `git ls-files`). In a listing the names are the content.
- Small outputs: under 1,200 estimated tokens for a table, 4,000 for a JSON document (8,000 for `Read`). A small JSON file is read for its values.
- Commands that ask for specific rows: `head`, `tail`, `sed -n`, `qsv slice`, and rtk's `--head-lines` and `--tail-lines`. Also commands that read the files dtk saved, so the drill-down commands in a summary return what they ask for.
- JSON documents that come out of a `jq`, `jaq`, `gron` or `yq` query: the model asked for those values.
- Output that rtk has already cut (`... (N lines truncated)`, `[see remaining: ...]`). A summary would claim to cover rows that are missing.
- Reads with `offset` or `limit`, and anything in `exclude_commands`.

## Works with rtk

rtk and dtk do different jobs.

| | rtk | dtk |
|---|---|---|
| Hook | `PreToolUse`: rewrites commands (`git status` becomes `rtk git status`) | `PostToolUse`: summarizes data in the output |
| Good at | known commands: git, cargo, tests, grep, ls | tables, CSV, JSON, logs |

`dtk init --global` registers **one** `PostToolUse` hook: `dtk hook claude --then rtk hook claude`. dtk looks at each event first. If the output is data, dtk answers. Every other event goes to rtk unchanged, byte for byte. dtk goes first because a table that rtk had already cut would be summarized wrongly.

What dtk does so that the two never get in each other's way:

- **`rtk init` keeps working.** rtk recognizes its hook by its shape. dtk's entry holds a silent placeholder named `rtk`, so `rtk init -g` sees its hook as installed and adds no second hook that could replace dtk's summary. `rtk init -g --uninstall` removes only the placeholder.
- **dtk sees through rtk's rewrites.** rtk turns `head -n 400 f` into `rtk read f --head-lines 400` and `cat f` into `rtk read f`. dtk still treats the first as a request for 400 rows, and `exclude_commands = ["^cat "]` still matches the second.
- **dtk leaves what rtk cut.** The pointer to the rest (`[see remaining: ...]`, `[+N hidden: rtk recall ...]`) stays in front of the model.
- **rtk's own `PostToolUse` entry is taken over** and put back exactly by `dtk init --uninstall`. rtk's `PreToolUse` hook is never touched.
- `dtk gain` shows dtk's totals and rtk's totals together. `dtk init --global --no-rtk` skips the chain, and dtk works alone if rtk is not installed.

All of this runs against the real rtk binary in CI (`tests/rtk_real.rs`, `tests/rtk_compat.rs`, and on Windows `tests/rtk_windows.rs`). The chain adds a few milliseconds: on one Mac, dtk alone took 2 to 7 ms, rtk alone 4 to 13 ms, and both together 17 to 21 ms.

## Commands

| Command | Effect |
|---|---|
| `dtk init --global [--no-rtk]` | Register the hook (and the rtk chain). |
| `dtk init --uninstall [--purge]` | Remove the hook and restore rtk's entry. `--purge` also deletes dtk's data and config. |
| `dtk mode shadow` / `dtk mode live` | Shadow decides and logs but changes no output. Live replaces output (the default). |
| `dtk doctor` | Check the hook, rtk, `qsv`, `jaq` and the config. Exits 1 on a warning. |
| `dtk verify` | Built-in PASS/FAIL checks of detection and decisions, then `doctor`. |
| `dtk explain FILE --command CMD` | Say whether dtk would summarize this output, and why. It uses the hook's own decision function. |
| `dtk discover [--since DAYS]` | Scan your Claude Code history for output dtk would have replaced, and what that would save. Changes nothing. |
| `dtk view FILE` | Show the summary of a file or of standard input. |
| `dtk gain [--json]` | Totals from the decision log, with rtk's totals beside them. |
| `dtk config [--create]` | Show the settings in use, or write a default `config.toml`. |

`DTK_DISABLED=1` makes dtk pass every output through (the rtk chain still runs). `DTK_RAW=1 cat data.csv` passes one command through.

## Config

`~/.config/dtk/config.toml` (create it with `dtk config --create`):

```toml
[hooks]
# "^text" matches the start of a command (or a file path for Read); any other text matches anywhere.
exclude_commands = ["^kubectl", "psql"]

[thresholds]
min_tokens = 1200          # smallest table worth replacing (estimated tokens)
budget = 2500              # largest summary
min_persisted_rows = 30    # fewest rows for output that Claude Code saved to a file
min_outline_tokens = 4000  # smallest JSON document worth an outline
```

A broken file means the defaults. The hook never fails because of a typo, and `dtk doctor` reports it.

## Safety

- **The hook fails open.** Any error means dtk adds nothing, and Claude Code keeps the original output. A chained command that is missing or runs longer than 10 seconds is skipped.
- It reads only files under `~/.claude/`, where Claude Code saves large outputs.
- A summary replaces the output only when it is clearly smaller: at least 30% fewer tokens for mid-size output, and always for output that Claude Code saved to a file.
- It does not touch MCP tool output.
- **Where data lives.** `~/.local/share/dtk/store/` holds full copies of replaced data (mode 0600 in a 0700 folder; the newest 60 files, nothing older than 14 days). `~/.local/share/dtk/decisions.jsonl` holds one line per decision, with the first 100 characters of the command. `~/.config/dtk/` holds the mode, the config and the record of what `dtk init` took over. Command output can contain secrets, so treat the store like the files Claude Code itself saves in `~/.claude/`.

## How it is checked

144 tests, and one script that uses a real model. The plan and its results are in [docs/PLAN-0.5.0.md](docs/PLAN-0.5.0.md).

| Check | What it proves |
|---|---|
| `tests/corpus.rs` | Real output of pandas, polars, sqlite3, tabulate, rich, jq, git, `gh api`, `ps`, CI logs and more: 50 files are detected as expected, 27 are rejected. |
| `tests/retention.rs` | The facts a summary must keep (row count, ranges, which row holds each extreme, value counts, first errors), computed by the test itself. 759 of 759 kept. |
| `tests/oracle.rs` | dtk's types, null counts, min, max, mean, medians and counts equal `qsv`, `jaq`, `awk`, `grep` and `sed`. |
| `tests/properties.rs` | Random text and bytes, and mutated real files, never panic and always give well-formed tables and smaller views. |
| `tests/rtk_real.rs`, `tests/rtk_compat.rs` | The real rtk binary: every init, re-init and uninstall order, and rtk's command rewrites and cut output. |
| `tests/outline.rs`, `tests/hook.rs`, `tests/tools.rs` | The hook against simulated Claude Code events, outlines, and the commands. |
| `scripts/ab-test.sh` | Real Claude Code with and without dtk (see above). |
| CI | macOS, Linux and Windows. The install routes are tested against each release by `install-test.yml`. |

## Limits

- Detection of whitespace-aligned text is a heuristic, and a wrong guess puts values under the wrong name. So dtk refuses what it cannot read safely: pandas `print(df)` (columns hidden behind `...`), `to_string()` with a datetime column (the labels could be wrong), and tables with column names that contain spaces, such as `netstat` and `lsof`. Print data with `df.to_csv()`, `df.to_json(orient="records", lines=True)` or polars.
- Inputs above 128 MB are skipped. A column with more than 5,000 different values shows its message shapes only if there are at most 200 of them; otherwise it shows the count.
- dtk is a separate program that calls rtk. It is not an rtk plugin: a plugin needs a custom rtk build, links rtk's source-available license, and would see only the first 30,000 characters of a large output.
- It works with Claude Code only. Windows has an x86_64 build (no ARM64 build yet).

## Use as a library

```rust
let ctx = dtk::lean::Ctx { bytes: 0, csv_path: None, raw_path: None };
if let Some((table, view)) = dtk::summarize(&text, &ctx, 2500) {
    println!("{} rows\n{}", table.rows.len(), view.text);
}
```

## Develop

```sh
cargo test
cargo clippy --all-targets -- -D warnings
scripts/ab-test.sh      # real Claude Code, with and without dtk (about 25 seconds per run)
```

Every test uses a temporary home folder.

## License

MIT. See [LICENSE-MIT](LICENSE-MIT).
