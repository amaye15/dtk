# Changelog

## 0.12.0

Out of the box, and more of the ways data gets printed. Found by checking what happens after an install, and by printing one DataFrame every way pandas allows.

- **The installers finish the job.** `install.sh` and `install.ps1` now run `dtk init --global` when Claude Code's folder (`~/.claude`) exists: the hook is registered (a backup is kept) and chained to rtk when rtk is installed. Nothing is written when the folder is not there, and `DTK_NO_INIT=1` installs only. Running the installer again is an update and leaves one hook. Before, the last line was "next: dtk init --global", a step to remember.
- **The hook survives an upgrade.** On Linux the operating system gives the real path of a program, and with Homebrew that is a versioned folder (`Cellar/dtk/0.11.0/bin/dtk`) that the next upgrade deletes. `dtk init` now writes the path from your `PATH` that holds the same file (`/home/linuxbrew/.linuxbrew/bin/dtk`).
- **Seven more ways pandas prints a frame, all read.** `print(df.to_dict())`, which is the default and has one dict per column keyed by the row number; `to_dict("index")`; `to_dict("split")`; `df.to_json()`, which gave an outline with no data before; `df.values.tolist()` (columns are called `c0`, `c1`, ...); one dict per line (`for r in rows: print(r)`); and `to_string()` of a frame with a **datetime column**, with an integer index or none, with labels that hold spaces (`New York`).
- **Datetime frames are read by position.** pandas right-aligns every column and a header ends where its values end, so the right edges of the header cut each line into cells, and a space inside a cell (a datetime, `Credit Card`) does not matter. A line that does not end at the last edge, or that has no space after an edge, makes the reader refuse the text. Two layouts that cannot be told apart are refused on purpose (a DatetimeIndex; a datetime in the first column with no index), because a datetime would be split into an index and a value. A line repeated 30 times, which has the same structure, is refused too: a table needs a padded header, a numeric column and rows that differ.
- Checked by: every value of each new shape compared with the pandas frame it was printed from (about 21,000 cells, none different); 9 new corpus fixtures with rows and headers taken from the pandas frames and 2 new refused layouts; the prose test that caught the repeated line; retention 2123 of 2123; install scripts run for real in four sandbox homes (folder present, folder absent, `DTK_NO_INIT=1`, a second run), and the same four cases are now in the install test of every release; two new real-model questions, Sonnet and Haiku 2 of 2 right with and without dtk, 78% and 81% fewer tokens.

## 0.11.0

Two wrong summaries, found by testing with the data that data people print. Both are fixed.

- **Small numbers keep their digits.** Statistics were rounded to four decimals, so a column of p-values, rates, losses or learning rates (1e-8 to 1e-3) read `rate: 0..0 mean 0 p50 0 p95 0`: a model would call the column all zeros. Now a number below 0.1 has four significant digits, a number below 1e-4 is in exponent form (`2.96e-8`), and the smallest and largest value are the cells as written (`rate: 2.960e-08..9.983e-06 mean 4.827e-6`). `dtk group` prints the same way. Cost: 20 tokens on a table with five such columns; 0.09% on the whole corpus.
- **pandas output with a MultiIndex is read.** `groupby([a, b]).agg(...)`, a Series from `value_counts()` or `groupby().sum()`, `pivot_table`, `set_index`. pandas leaves a repeated index label blank, so the rows do not have the same number of cells and a split on spaces gives the wrong columns. The new reader works by character position instead: the line of index names marks the index columns, and the right edges of the values mark the value columns. Two or three lines of column names are joined (`amount.sum`, `qty.count`), a blank top label repeats the one to its left, a label with a space (`New York`, `Home & Garden`) stays whole, and the name of a column level (`year` in a pivot table) is not a column. A Series needs its `Name: ..., dtype: ...` footer. A line that does not fit makes the reader refuse the text, not guess.
- **Fixed: a table with a missing column and missing rows.** When the groups were big, the aligned-text reader took the short rows (one blank index label) for the whole table. It dropped the first row of every group and the group column, and the summary still said "statistics over all rows". A 240-row `groupby` was read as 236 rows with no `region`. The reader now refuses a table when a line it would drop has more cells than the rows it keeps.
- Checked by: every value of 20 real pandas outputs (about 11,000 cells, with row counts and header names) compared with the pandas frame they were printed from, none different; 8 new corpus fixtures whose rows and headers come from the pandas frames, and one of tiny values; a test that fails without the fix to the aligned reader; retention 1854 of 1854 (the new fixtures are what found the rounding); three new real-model questions, Sonnet and Haiku 2 of 2 right with and without dtk, 78% to 80% fewer tokens.

## 0.10.0

Fewer tokens for more information, from reading what the summaries of real API data spent their tokens on. All automatic.

- **Nested JSON is opened.** An object that most rows have, with the same few keys (an `owner` with a `login` and an `id`), becomes columns: `owner.login`, `owner.id`, up to two levels. Before, each was one cell of JSON text, shown as `{"login":"chenyukang","id":230646,"node_...` with no statistics. A map whose keys are data (more than 20 keys), a key that is sometimes a string, and a table that would pass 150 columns stay as they were.
- **Columns that say nothing get a name, not a line.** Same value in every row (and no gaps): one `same in every row: a=1, b=x` line (the rest are named). A different token in every row, with no spaces (ids, URLs, node ids, hashes): one `different in every row` line with one example of the first two. Columns that split the rows into exactly the same groups as an earlier column (the id, login, node id and URL of one actor): `same groups as actor.login: actor.id, actor.url, ...`. A list cut short says `+N more`. Statistics lines and sample rows go to the columns that vary; the sample rows say which columns they show.
- **Hashes by kind.** 40-character hex ids and UUIDs are `16 distinct, 40-character hex ids`, not two examples. A list of long values stops when it is long (`+N more`).
- **Sample rows that earn their place.** Fewer plain first and last rows (5 and 2, fewer when rows are wide), and the **full rows of the extremes** after them: the largest and smallest of the first measure and the largest of the second. The model sees who and where the largest amount was.
- Across the 63 corpus fixtures that get a summary, the summaries are 6.8% smaller than 0.9.0's (28,156 to 26,244 tokens), real API JSON by up to 20%, and they hold more.
- Checked by: the retention harness now asks of every column that has no statistics line whether the summary accounts for it truthfully, working it out from the raw cells (a column called constant must be constant, a copy must have identical groups, a unique column must be unique): 1657 of 1657, including the full row of the largest value of the first measure.

## 0.9.0

Back to the point of dtk: minimum tokens, maximum information, automatically, with rtk. 0.8.0 added `dtk group` as a command, but a command is a second step that the model must decide to take. The useful part belongs in the summary.

- **Group means in the summary.** For the leftmost label column (a region, a status, a flag, an HTTP code; 2 to 8 values, groups of 5 rows or more) against the leftmost measure, the summary shows the mean per group: `amount by region (mean): ap 166.8, us 161.8, eu 101.5`. The splits that explain the most variance are added (the strongest of the rest at a tenth, others at a quarter), at most three lines. A model asked for the mean amount for one region now answers from the summary alone: 3 turns per question became 2 (Sonnet, 3 of 3 right; Haiku 3 of 3 right). Checked against group means computed independently by the retention harness (1298 of 1298 facts).
- **Same token cost.** The `drill:` line is shorter now (three examples with real column names, and a pointer to the rest). Net change against 0.8.0: 1 to 10 tokens per summary.
- **rtk.** Verified with the real rtk that its pre-command hook does not rewrite `dtk ...` commands, and that its post-command pipeline, which cuts a long output to about 250 lines, would cut a long result of a dtk command. dtk now keeps its own command results out of that pipeline. A real-rtk test runs the pipeline on an ordinary long output (it is cut, so the test proves something) and on dtk's commands (they are not).

## 0.8.0

The analytics a data person asks first, built in so that no other tool is needed.

- **`dtk group FILE BY [MEASURE...]`.** The mean, sum, min and max of each measure per value of BY (`region,tier` for two columns), the biggest groups first. `--agg mean,sum` picks some, `--limit N` caps the groups. It reads `$1,200` and `45%` as numbers and skips `N/A`. Since 0.6.0 the README said `qsv` was not needed, but "mean of X per Y" was the one common question without a built-in answer. The `drill:` line now suggests it.
- **Time columns.** A summary says whether the rows are in time order (`sorted oldest first`, `sorted newest first`, `not in time order`), the step between them (`every 5min`, or `typical step ...` when uneven), and the longest gap when it is more than three steps (`longest gap 3h5min after row 148`). Times are read with their offset (`Z`, `+02:00`) and fractions of a second. Dates alone give `every 1d`.
- **Correlated columns.** Pairs of measure columns with |Pearson r| of 0.9 or more are named, strongest first (`temp_c~temp_f 1.00`). Identifier columns and codes are left out, because a column that rises with the row number correlates with anything that drifts. A table over 100,000 rows is sampled at an even stride.
- Checked by: a time-series fixture with a known outage (`sensor_readings.csv`), harness facts computed from the raw cells (sortedness of every ISO time column in the corpus, and every correlated pair), a test of `dtk group` against a group-by made in the test, and two real-model questions (mean per group, interval and gap of a sensor log): Sonnet and Haiku 3 of 3 right with and without dtk, with 88% and 85% fewer tokens. Retention 1204 of 1204.

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
