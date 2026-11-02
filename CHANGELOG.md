# Changelog

## 0.7.0

More of the shapes data takes, found by printing a DataFrame every way pandas allows and checking what dtk made of each.

- `df.to_csv()` was not read. It writes the index as a column with an empty name, and the empty header made dtk refuse the file. That column is now called `index`.
- **Python repr.** `print(df.to_dict("records"))` and a dict of lists (single quotes, `True`, `None`, `nan`, `Timestamp('...')`, `np.float64(1.5)`; any other call stays as its text). Valid JSON is never read this way.
- **Columnar JSON.** `to_json(orient="split")` (`{"columns": [...], "data": [[...]]}`, also `headers`/`rows`) and an array of arrays whose first row names the columns.
- **Number grids.** A printed numpy matrix, `np.savetxt`, any rows of numbers with no header (columns `c0`, `c1`, ...).
- **Headerless delimited text.** `psql -t -A`, `sqlite3 -list`, `cut`. There are no names to check, so the cells must show it is data: the same field count on 98% of the lines, three or more fields, a third of the columns numbers. A drawn table (`| 1 | 2 |`) is never read this way.
- **Record blocks.** `psql -x`, `sqlite3 -line` and MySQL `\G`, one block per row; every block must have the same keys.
- **HTML tables.** A page that is one `<table>`, as `df.to_html()` writes it. A full page or two tables is left alone.
- **Dates.** `03/21/2025` and `21.03.2025` are dates when the data settles the order (a part above 12 says which is the month), with a range in ISO form. A column where every part is 12 or less stays text, because a wrong guess would give a wrong range.
- A test that said "numbers only is not a table" now expects a grid of numbers; the fixture moved to the positives on purpose.
- Checked by: 12 new corpus fixtures and 4 new negatives (a Python config dict, one number per line, an IP/port list, prose with commas), retention 1148 of 1148, and two new real-model questions (a Python list of records, a pandas JSON file): Sonnet 3 of 3 and Haiku 3 of 3 right with and without dtk on each, with 82% and 88% fewer tokens.

## 0.6.0

Made for data work: real-world columns, data-quality notes, wide tables, and a drill-down that needs no other program.

- **Placeholders.** `N/A`, `NA`, `NaN`, `null`, `none`, `-`, `?` and a few more in a column of numbers, dates or flags are counted and set aside (`31 placeholders excluded (- 11, N/A 10, ...)`). The column keeps its range, mean and quantiles. Before, one `N/A` made the whole column text.
- **Numbers as people write them.** `$1,234.56`, `€5`, `45.3%`, `1,234` and `(12.50)` are numbers, with one way of writing per column, and the summary says how. A leading zero is never a number (`02134`), and `1,5` is not read as a thousands separator.
- **Data-quality notes.** Outliers beyond three interquartile ranges are named (a few values; a heavy tail is not outliers). Rows that repeat an earlier row are counted. Identifier columns (`order_id`, `uuid`, a column that rises at every row) no longer get a largest and a smallest.
- **Wide tables.** Every column is named, and a run of counter columns is one range (`feat_0..feat_119:float`). Statistics cover the first 30 columns, or a dozen for a table of more than 40 columns, without p50 and p95. A 62-column table went from 1,771 tokens to 572.
- **Drill-down without `qsv` or `jaq`.** `dtk rows`, `top`, `count`, `where`, `stats` and `get` read the data behind a summary. The `drill:` line uses them, with real column names and the short id of the data file (`dtk top 5fa6aeae75bd amount 10`). `dtk top` and `where` read `$1,200` and `1200` as the same number, and `> 5` is false for `N/A`. `dtk get` reads `.a.b[0]`, `.items[]`, `| length` and `| keys` from a JSON document. `qsv` and `jaq` are optional extras now, and `dtk doctor` no longer warns when they are missing. If `dtk` is not on the PATH of the shell, summaries use its full path.
- dtk's own drill-down commands are never summarized again.
- **Correction to the 0.5.0 notes.** They said that Haiku sometimes wrote about the tool and did not give the answer, that a longer summary header made this worse, and that Sonnet gave 19 of 20 right answers with dtk. All of that came from a mistake in `scripts/ab-test.sh`: it read only the last line of the model's answer, and with dtk the model often adds a note about the summary on a new line after the answer. With the whole answer read, **Sonnet is 26 of 26 right with dtk and 26 of 26 without (13 questions, 2 runs), and Haiku is 39 of 39 and 39 of 39 (3 runs)**. The shorter summary header stays because it is shorter, not because of that test.
- Checked by: the retention harness (1018 of 1018 facts, now including placeholders, formatted ranges, outliers and repeated rows, computed by the test itself), an `awk`/`qsv` oracle on a messy fixture (count, min, max, mean and placeholder count of four columns, and the repeated rows), nine end-to-end tests of the drill-down commands against sorting and counting done in the test, and three new questions in the real-model test (`scripts/ab-test.sh`).

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

- Repeated errors in a text log are grouped by the shape of their message, with counts and the first of each shape with its context (a 57,000-line macOS log: 148 errors in 13 shapes). Message shapes are now counted over every row when there are more than 5,000 distinct messages (they stopped there before).
- The summary header is shorter and states no more than it must: `(statistics over all rows; the full data is on disk)`. A longer wording made Haiku describe the tool instead of answering in about 5 of 8 runs.

Not detected on purpose: directory listings such as `ls -l` and `find` (the names are the content).

Measured with real Claude Code (`scripts/ab-test.sh`: ten questions, each asked with and without dtk; the true answers come from jaq, qsv, awk and grep):

- Sonnet, 2 runs per case and arm: tokens that tool results put in front of the model fell 70% to 90% on seven cases (6% and 11% where Claude Code's saved-file preview is already small), and turns fell from 6 to 4 on five. 19 of 20 answers right with dtk, 20 of 20 without; the one miss did not repeat in 5 more runs.
- Haiku, 3 runs per case and arm: the same savings; two cases gave a wrong answer once or twice with dtk. One was a question with a tie in the data, graded case-sensitively; in the other run Haiku wrote about the tool instead of the answer. Sonnet did not do that in 6 of 6 runs.

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
