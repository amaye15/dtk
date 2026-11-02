# Changelog

## 0.5.0

Compatibility with rtk (found by running the real rtk 1.1.0; `tests/rtk_compat.rs`):

- rtk rewrites `head -n 400 f` to `rtk read f --head-lines 400`. dtk did not know `--head-lines` and summarized the 400 rows that were asked for. It now sees through rtk's rewrites.
- `exclude_commands = ["^cat "]` stopped matching once rtk rewrote `cat f` to `rtk read f`. Patterns are now matched against the command as typed and as rewritten.
- A table that rtk had cut (`... (N lines truncated)` with a pointer such as `[see remaining: ...]` or `[+N hidden: rtk recall HASH]`) was summarized as "all rows", and the pointer to the rest was lost. dtk now leaves output that rtk cut.

Summaries that keep more of what questions need:

- Each numeric column says which row holds its largest and smallest value, named by id (`max at order_id=100243, min at order_id=100136`). Columns of up to 12 values list all of them (they stopped at 8). About 5% more tokens per summary.
- `tests/retention.rs` lists the facts a summary must keep (row count, column names, ranges, which row, value counts, time span, first and last row, the first errors), computes them with its own arithmetic, and checks the summary text: 564 of 742 facts kept before, 100% after.

New formats:

- `ps aux`, `ps -ef`, `df -h`: tables whose last column is free text. The parse is strict, because a wrong guess puts values under the wrong names. Real `ps aux`: 453 rows, 38,148 tokens to 739, every cell equal to an independent split.
- Plain text logs that start with a timestamp (GitHub Actions, application logs, syslog). Errors are shown with the lines before them. Real failed CI job: 21,703 tokens to 569, and the summary shows the cause, which Claude Code's preview (the first 550 tokens, all runner set-up) did not.
- JSON documents that are not tables get an outline (see the README). `cargo metadata`: 37,913 tokens to 271.
- Fixed: a JSON object with a small array of objects beside a big object (40 outputs next to 120 resources one level deeper) was summarized as the small array, and the rest was dropped. The rows must now be 70% of the document.
- Fixed: a panic on multi-byte text in the new log parser, found by the fuzz tests.

Not detected on purpose: directory listings such as `ls -l` and `find` (the names are the content).

## 0.4.0

- Every install way that rtk has: the `install.sh` script (now with `DTK_VERSION`, `DTK_INSTALL_DIR` and `DTK_SKIP_CHECKSUM`, and a refusal to install a file whose SHA-256 sum does not match), an `install.ps1` for Windows, a Homebrew tap (`brew install amaye15/tap/dtk`), a Scoop bucket, winget manifests, pre-built archives for macOS, Linux (static musl, x86_64 and ARM64) and Windows, `.deb` and `.rpm` packages, and `cargo install`. The guide is `docs/INSTALL.md`.
- Windows support, tested in CI on a real Windows runner with the real `rtk.exe`: the hook and the rtk chain, paths with spaces or backslashes, and the placeholder that keeps `rtk init` working.
- `dtk init --uninstall --purge` removes dtk's data and config as well. `dtk init --uninstall` prints the command that removes the program for the way you installed it (`brew uninstall dtk`, `cargo uninstall dtk`, `scoop uninstall dtk`, and so on).
- A workflow (`install-test.yml`) installs the newest release by every route on the system that route is for, and runs `dtk verify`. The release workflow builds the five binaries and the four packages, installs the `.deb`, and removes it again, before it uploads anything.
- Fix: the release workflow could try to upload to a release that did not exist yet.

## 0.3.0

- More formats: JSON objects that wrap one big array (also two levels down) or are keyed by id, logfmt, Apache and nginx access logs, JSON lines behind a prefix (`docker compose logs`, `kubectl logs --prefix`). Wide tables (83 columns of nested JSON) get a summary of their first columns.
- Cleaner input: ANSI colour codes, a byte-order mark, `#` comment headers, header rule rows (sqlite `-column`, tabulate `simple`), and the whole box-drawing range (rich).
- Smaller summaries: text with many distinct values is grouped by message shape with counts; a long saved-file path gets a short link, which cuts about 20% from the summary of a saved-to-file CSV.
- Full rtk compatibility. `rtk init -g`, run again after dtk, used to add a second `PostToolUse` hook beside dtk's. dtk's entry now holds a silent placeholder named `rtk`, so rtk sees its hook as installed. Tested against the real rtk 1.1.0 in every init, re-init and uninstall order, on macOS and Linux.
- New commands: `dtk explain`, `dtk verify`, `dtk discover`, `dtk config`, `dtk gain --json`, and an `install.sh` that checks the SHA-256.
- `~/.config/dtk/config.toml`: `exclude_commands` and thresholds.
- Verification: a corpus of real tool output with a detection matrix, property tests, an oracle against `qsv`, `jaq`, `awk`, `grep` and `sed`, and an A/B script with real Claude Code. See `docs/PLAN-0.3.0.md`.
- Fixes found on the way: a `-` in an access log's bytes column made it text; a wide table with few rows was shown in full; a tiny table could get a summary larger than itself; saved-to-file outputs were skipped by `discover`.

## 0.2.0

- Summarize data files that Claude reads with the `Read` tool (whole-file reads of `.csv`, `.tsv`, `.jsonl`, `.json`, `.log`, `.txt` and a few more). Reads with `offset` or `limit` pass through.
- Read tables drawn with borders: polars, psql, MySQL, SQLite and Markdown.
- Add the median (p50) and p95 to numeric columns.
- Never summarize the output of commands that ask for specific rows (`qsv slice`, `head`, `tail`, `sed -n`, rtk's `--max-lines` and `--tail-lines`). `DTK_RAW=1` passes one command through.
- Refuse pandas `to_string()` tables when the column labels could be wrong (a datetime column joined to its neighbour by one space). This fixes mislabelled columns.
- `dtk init` now registers the `Bash|Read` matcher. `dtk doctor` warns about registrations made before 0.2.0.
- The rtk chain waits for the chained command with short checks at first (0.2 ms, backing off to 10 ms) instead of a fixed 10 ms sleep.

## 0.1.1

- A closed output pipe (`dtk view big.csv | head`) ends quietly instead of printing a panic.

## 0.1.0

- First release: summaries of JSONL, JSON arrays, CSV, TSV and aligned tables in Bash output; the rtk chain; `dtk init`, `doctor`, `gain`, `view` and `mode`.
