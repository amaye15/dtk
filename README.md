# dtk: data token killer

`dtk` is a Claude Code hook that works together with [rtk](https://github.com/rtk-ai/rtk). It finds big tables in Bash output and swaps them for a short summary that covers every row. The full data stays on disk, so the agent can query it.

It is one small Rust binary (about 600 KB) with no runtime dependencies.

## What it does

Claude Code sends the output of a Bash command to the model. For `cat data.csv`, `python report.py` or `tail -n 500 app.log`, that output can be tens of thousands of tokens of rows. `dtk` runs after the command (a `PostToolUse` hook) and checks whether the output is a table. It also checks data files that Claude reads with the `Read` tool.

| Input found in the output | Treated as |
|---|---|
| JSON Lines (one object per line) | `jsonl` |
| JSON array of objects | `json-array` |
| CSV, or TSV, `;` and `\|` separated text with a header row | `csv`, `tsv` |
| Tables drawn with borders: polars, psql, MySQL, SQLite, Markdown | `table` |
| Whitespace-aligned tables, such as `DataFrame.to_string()` | `table` |

Anything else passes through untouched.

For a table of 60 rows or fewer, `dtk` re-encodes it as CSV and drops nothing. For a bigger table, it shows:

- the shape and the size
- the column names with types
- per-column statistics: min, max, mean, median (p50) and p95 for numbers; value counts for categories and codes; the time range for timestamps; null counts
- the first rows and the last rows
- the rows where a `level` or `severity` column says warn or error
- the exact commands to query the full data, with `qsv` and `jaq`

Measured with OpenAI's `o200k_base` tokenizer, as a stand-in for Claude's:

| Input | Raw tokens | With dtk |
|---|---:|---:|
| 5,000-row CSV, 8 columns | 171,244 | 855 |
| 400 JSONL log lines | 20,481 | 1,549 |
| JSON array, 200 records | 11,055 | 896 |
| Aligned table, 200 rows | 7,872 | 805 |
| CSV, 200 rows | 6,871 | 842 |

Claude Code already saves outputs of 30,000 characters or more to a file and shows the model a preview of the first lines. `dtk` reads that saved file, so its summary covers all rows, not only the first lines.

**The `Read` tool.** When Claude reads a whole data file (`.csv`, `.tsv`, `.jsonl`, `.json`, `.log`, `.txt` and a few more), the model gets the summary instead of every row. A read with `offset` or `limit` is a request for exact lines, so it is left alone. Source code is never checked. `dtk` works on the result after `Read` ran, so it cannot show anything that your permission rules blocked. Claude Code refuses `Read` on files over 256 KB; use `cat` or `dtk view` for those.

## Install

Download the archive for your system from the [releases page](https://github.com/amaye15/dtk/releases), check the `.sha256` file, and put `dtk` on your `PATH`. Or install from crates.io, or build it:

```sh
cargo install dtk
cargo install --path . --locked     # from a clone
```

Then register the hook once and start a new Claude Code session:

```sh
dtk init --global       # edits ~/.claude/settings.json, writes a backup first
dtk doctor              # checks the hook, rtk, qsv and jaq
dtk init --uninstall    # removes the dtk entry and restores rtk's own hook
```

`dtk init` keeps every other setting and every other hook. Hooks load when a session starts, so a running session does not change.

For the drill-down commands in the summaries, install `qsv` and `jaq` (for example `brew install qsv jaq`).

## Working with rtk

rtk and dtk do different jobs and fit together:

| | rtk | dtk |
|---|---|---|
| Hook | `PreToolUse`: rewrites commands (`git status` becomes `rtk git status`) | `PostToolUse`: summarizes tables in the output |
| Good at | known commands: git, cargo, tests, grep, ls | tables, CSV, JSONL, data frames |
| Optional post-command pipeline | `rtk:posthook` compresses other output after the command | goes first, because it needs the raw table |

**One hook, a fixed order.** `dtk init --global` registers a single `PostToolUse` hook: `dtk hook claude --then rtk hook claude`.

1. dtk looks at the event first. If the output is a table, dtk answers, and rtk is not called.
2. Every other event goes to rtk unchanged, byte for byte. rtk's answer and exit code come back as they are.

dtk must go first. A table that rtk had already cut to its first and last lines would be summarized wrongly.

**rtk's own hook is folded in.** If rtk already has a `PostToolUse` entry in `settings.json` (for example after you enabled `rtk:posthook` and ran `rtk init -g`), `dtk init --global` takes over that registration, keeps its matcher, and runs rtk's hook through the chain. Two hooks would otherwise run in parallel and fight over one output. `dtk init --uninstall` puts rtk's entry back exactly. If you run `rtk init -g` again later, run `dtk init --global` again too. `dtk doctor` warns when the two run side by side.

**rtk's `PreToolUse` hook is never touched.** The commands rtk rewrites (`rtk read data.csv`) still produce tables that dtk summarizes.

**Shared view.** `dtk gain` prints dtk's totals and rtk's totals (`rtk gain`) together.

Use `dtk init --global --no-rtk` to skip the chain. If rtk is not installed, dtk works alone.

The two programs run one after the other, so the hook takes about the sum of both. Measured on one Mac, as medians: dtk alone 2 to 7 ms (about 2 ms for a typical event, about 7 ms for a 331 KB CSV), rtk alone 4 to 13 ms, both through the chain 17 to 21 ms. That is small next to a model call.

Tested end to end with real rtk 1.1.0 and Claude Code: with `rtk:posthook` enabled, a 150-row JSONL log became a dtk summary (864 tokens), and an 850-line plain text output went through the chain to rtk, which cut it to 252 lines and added its `[full output: ...]` pointer.

## Control

| Command | Effect |
|---|---|
| `dtk mode shadow` | Decide and log, but change no output. Use it to see what `dtk` would do. |
| `dtk mode live` | Replace output (the default). |
| `DTK_DISABLED=1` | dtk passes every output through. The rtk chain still runs. |
| `DTK_RAW=1 cat data.csv` | Pass this one command's output through. |

Commands that ask for specific rows are never summarized: `qsv slice`, `qsv sample`, `head`, `tail`, `sed -n`, and rtk's `--max-lines` and `--tail-lines`. That includes the `rows:` command in a summary.
| `dtk gain` | Totals from the decision log, with rtk's totals beside them. |
| `dtk view FILE` | Show the lean view of a file or standard input. |
| `dtk doctor` | Check the setup. |

## Safety

- The hook fails open. Any error means no output from dtk, so Claude Code keeps the original output. A chained command that is missing or runs longer than 10 seconds is skipped.
- It only reads files under `~/.claude/`, the folder where Claude Code saves large outputs.
- A summary is used only if it is clearly smaller: at least 30% fewer tokens for mid-size output, and always for output that Claude Code saved to a file.
- Strict detection: prose, source code, single JSON objects, cargo output and numeric headers are not tables.
- It handles Bash output and whole-file `Read` of data files. MCP tools are not touched.

## Where data lives

- `~/.local/share/dtk/store/` holds full copies of replaced data (CSV, and raw JSON or JSONL). Files are mode 0600 in a 0700 folder. The store keeps the newest 60 files and nothing older than 14 days.
- `~/.local/share/dtk/decisions.jsonl` holds one JSON line per table found: format, rows, token counts, the decision, and the first 100 characters of the command. It stays under about 4 MB.
- `~/.config/dtk/` holds the mode and the record of what `dtk init` folded in.

Command output can contain secrets. Treat the store like the files that Claude Code itself saves in `~/.claude/`.

## Use as a library

```rust
let ctx = dtk::lean::Ctx { bytes: 0, csv_path: None, raw_path: None };
if let Some((table, view)) = dtk::summarize(&text, &ctx, 2500) {
    println!("{} rows\n{}", table.rows.len(), view.text);
}
```

## Limits

- Detection of whitespace-aligned text tables is a heuristic. The default `print(df)` of pandas hides columns behind `...`, so `dtk` does not treat it as a table. A pandas `to_string()` table with a datetime column is refused too, because pandas can join that column to its neighbour with one space and the column labels could come out wrong. Print with `df.to_csv()`, `df.to_json(orient="records", lines=True)` or polars, which `dtk` reads correctly.
- Inputs above 128 MB are skipped.
- Statistics cover the whole table. For columns with more than 5,000 distinct values, only a count is shown.
- dtk is a separate program that calls rtk. It is not an rtk plugin: a plugin needs a custom rtk build, links rtk's source-available license, and would see only the first 30,000 characters of a large output.
- Windows is not supported yet.

## Develop

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

The unit tests cover detection and rendering. `tests/hook.rs` runs the real binary against simulated Claude Code events (including real pandas output in `tests/fixtures/`), and `tests/rtk_integration.rs` tests the rtk chain, the folding and the restore with a fake `rtk`. All of them use a temporary home folder.

## License

MIT. See `LICENSE-MIT`.
