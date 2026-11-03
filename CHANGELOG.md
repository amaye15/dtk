# Changelog

## 0.16.0

Web pages from the two other places they come from: MCP tools and files that are read.

- **MCP tools.** A page returned by a fetch or browser MCP server (`mcp__fetch__fetch`, a "page HTML" tool) becomes its text in the shape it came in (content blocks stay blocks, a string stays a string). An MCP tool that reads files from the disk (`read_file`, `read_text_file`, `get_file_contents`, a `filesystem` server) is left alone, because for an HTML file that may be source being edited. Live pages are not checked for template syntax, because they often hold `{{` in a script or a code sample. `exclude_commands = ["^mcp__fetch__"]` skips a server. Checked with real Claude Code and a test MCP server: a 4,920-token page became 391, with the same answer.
- **Files read with `Read`.** A whole `.html` file that is a page is shown as its text, with a line that says what it is and how to see the markup (read it with `offset` and `limit`, or use `cat`). Left alone: a range, a file under about 10 KB (2,400 tokens, source being worked on), a file with template syntax (`{{`, `{%`, `<%`, `<?php`, Vue and Angular directives, Alpine's `x-data`), and any name that is not `.html`, `.htm` or `.xhtml`. Setting: `html_reads` (it also governs MCP). A 5,144-token file became 467 with the same answer.
- **Edits still work.** The case that could go wrong is a model that reads a big HTML file in order to change it. With real Claude Code on Sonnet, asked to change the `<title>` of a 20 KB page after reading it, the edit was correct and minimal in all four runs (two with dtk, two without). With dtk the model either made the edit directly or looked at the markup first with `Grep` or `Bash`, as the note says, and each run cost about 30% less because the raw page never entered the conversation.
- Checked by: tests for each path and each guard through the real binary (Read, a range, a small file, five kinds of template, another extension, the setting off; MCP in both shapes, three file-reading tool names, a live page with `{{`, a small page, an exclusion); `scripts/mcp-test.sh` and `scripts/tools-test.sh` have a page case each; 252 tests.

## 0.15.0

Two kinds of output that are long and mostly noise: build and test logs, and web pages.

- **A log digest.** Claude Code saves an output of more than 30,000 characters to a file and shows the model its first lines, which in a log are the boring ones. Now a log that is saved gets a digest of all of it: what it is made of (`3200 x compiling *`), every kind of error and warning with the line number of the first, where the others are, and the lines that follow it when a compiler puts the place under the message; how many errors there are and in which files; the first lines and the last twelve, where test runners say how it ended. It is made by counting, not by a model, so it is instant, free and the same every time. A real `pytest -v` run of 13,000 tokens became 650, with `7 failed, 593 passed` and the line of every failure; 80 rustc errors of 13,800 tokens became 660, with each error code counted. The full text stays in the saved file, and the digest says how to read part of it.
- **It is for logs, and only logs.** Source code, documents, diffs, lists of names and results of a search are the lines themselves. A digest needs the output to look like a log in two ways (a few line shapes make up most of it, and most of its lines say something a log says: a level, a timestamp, `Compiling`, `test ... ok`) or to be a pile of diagnostics that begin with `error`, `warning` or `path:12:5: error:`. It is never made for `cat`, `grep`, `find`, `git diff`, `curl` and their kind. Checked against every big output in a 60-day history and every source, document and rejected fixture of this repository: none is taken for a log. Setting: `digest_logs`.
- **A web page becomes its text.** A page fetched with `curl` or `wget` (or a script with an address in it) is 85% to 90% markup: Wikipedia's CSV article is 48,000 tokens of HTML and 2,300 of text. dtk gives the model the page as a person reads it: headings, paragraphs, lists, tables, code, and links with their addresses, without scripts, styles, SVG, hidden elements and the menu (`(navigation menu: 35 links)`). The page is saved, and `dtk html ID text|headings|links|tables|json|grep` reads any part of it, including JSON that the page embeds. A comparison with an independent walk of the page found none of its visible words missing. A page that is a table (`df.to_html()`) is still a table. `cat page.html`, `sed`, `grep` and `Read` are people working on the markup, and are never converted. Setting: `html_pages`.
- **Fixed in the text-log summary:** a timestamped log's error rows were cut to the width of a sample row and gave no way to find them. They are now cut at 170 characters and say which row they are (`row 1201: ...`). Before this a model asked for the first error and its line could only say it was cut off.
- Checked by: real logs from pytest, cargo test and rustc as fixtures, with the facts worked out in the test; a rule that nothing in this repository that is not a log is digested; hook tests for a saved event (digest, the commands that must not get one, the setting); a real web page as a fixture with the words counted by an independent walk; real Claude Code on Sonnet (about 10% cheaper and one turn fewer on logs, and right 10 of 10 where the plain arm was 9 of 10; 31% to 40% cheaper on a page of 5,000 tokens, and 8% to 15% cheaper on pages of 25,000 to 48,000 tokens, saved to a file). Tried and not built: a message suggesting a smaller model, which a model never took in 16 sessions and which cost about 7% more.

## 0.14.0

How dtk meets the tools that Python and CI work runs through: `uv`, rtk's wrapper around it, and the GitHub CLI.

- **rtk cut a script's output to fifty rows; dtk now reads what rtk saved.** rtk's pre-command hook rewrites `uv run python script.py` to `rtk uv run python script.py`, and rtk cuts the output of that wrapper to about fifty lines, with a pointer to the rest. A script that printed an 800-row table reached the model as 50 rows, and dtk, which refuses to summarize output that rtk has cut, could not help either. rtk keeps the whole output. dtk now reads it: the tee file named in `[see remaining: tail -n +26 "$HOME/.../rtk/tee/1_uv-run-stdout.log"]`, or `rtk recall --full HASH` for `[+N hidden: rtk recall HASH]`. A 500-row DataFrame with a datetime column, printed with `uv run`, went from 50 rows in 919 tokens to a summary of all 500 rows in 452. With real Claude Code and the real rtk hook: Sonnet and Haiku right 2 of 2 with and without dtk; 4 turns instead of 6, a third fewer tokens. The safeguards: only a file inside a folder named `rtk/tee` and ending `.log`; its first line must be the first line of what rtk showed; a copy that rtk truncated at its 1 MB limit (it ends `--- truncated at ... ---`) is not used, because a summary of it would say "all rows"; `rtk recall` gets three seconds. When no complete copy can be read, the output is left alone as before. The same applies to `rtk jq . big.json`, which rtk also cuts.
- **uv's progress lines do not hide the table.** `uv run script.py 2>&1` on a cold cache prints a dozen lines (`Downloading pandas`, ` Downloaded pandas`, `Installed 13 packages in 79ms`, ` + numpy==2.5.4`) before the script's own output, which pushed it past the six lines of text the wrapped-data reader allows. Progress lines of `uv` and `pip` are now allowed in addition to the six (up to 80), and stay in the output as they were. A DataFrame printed after them became a summary: 3,490 tokens to 366. Without `2>&1` uv writes them to stderr, which dtk does not touch.
- **GitHub CLI output.** Not on a terminal, `gh` prints tab-separated lines with no header, so nothing in the text says what a column is. The command does: `gh run list` has nine fields, `gh pr list` five. Now `gh run list`, `pr list`, `issue list`, `release list`, `repo list`, `search repos|issues|prs`, `pr checks`, `workflow list` and `label list` are tables with the names of their fields, and `gh run view --log` and `--log-failed` are logs with a `job` and a `step` column and the errors shown (a 274 KB log of three jobs became about 500 tokens; a failed run's log names the failing test). The command is read before the loose readers, because a three-column listing looks like a table whose first row is the header. `--json`, `--jq` and `--template` mean the user shaped the output, and then none of this applies.
- Checked by: 9 real or layout-true `gh` fixtures with the command that made each; a real `uv run` cold-cache output as a wrapped fixture; unit tests for the tee and recall rules (not rtk's folder, not a `.log`, a truncated copy, a different output, a missing file); a hook test with a tee file; real-rtk tests (rtk cuts a document, dtk summarizes it from rtk's copy, and leaves it alone from a home that has no copy); 226 tests; `scripts/gh-test.sh` with a stand-in `gh`.

## 0.13.1

Four wrong readings, found by lining up every big output in a real 60-day history against what the readers made of it (`cargo run --release --example audit -- FILES` does this for any set of outputs). All four are older than 0.13.0; widening dtk to MCP tools, whose results include documentation pages, is what brought the first one to light.

- **A document with a table in it was replaced by the table.** A 160-line page with a 13-row Markdown table became those 13 rows, and the other 147 lines were dropped without a word. File lists and `grep` output with an aligned part did the same. Now every line a reader does not use as a row, header, border or kept note is counted. More than five such lines, and more than a tenth of the rows, means the text is not a table and passes through. Fewer are listed in a note: `9 other lines are not rows and are not shown`.
- **An `ls -l` listing was read as a table whose header was a file.** The names are the content of a listing, so it is left alone (80% of lines in the `-rw-r--r--` form).
- **A header cell that is a piece of data is not a name.** A number, a space and more (`8 andrewmayes`); a date or timestamp (`2026-08-04T22:01:17Z ...`, also behind a byte-order mark); half of a bracket (`layers.0.weight (128` and `)`). The tab-separated CI log from `gh run view --log` had no header, so its first line became the column names; a list of `name (128, 1)` lines was split at its comma.
- Of 492 outputs read as tables in that history, 20 were wrong and are now left alone; the rest were checked column name by column name. The 20 had counted for about 20,000 tokens of "savings".
- Checked by: 5 rejected fixtures, each shaped like a real output (with names changed) that 0.13.0 reads as a table and 0.13.1 does not; unit tests for the rules; 216 tests.

## 0.13.0

Data that arrives with text around it, and data that comes from MCP tools.

- **MCP tool results.** The hook now matches `mcp__.*` as well as `Bash` and `Read`. A table that an MCP tool returns (rows from a database server, a list of issues) is replaced by its summary, in the shape Claude Code gave it: content blocks stay content blocks, and structured output stays a string. A big JSON document that is not a table is left alone, because the model asked a server for those values. Results with an image, and results that are errors, are left alone. `exclude_commands = ["^mcp__github__"]` skips a server; `dtk explain --mcp NAME`, `dtk discover` and `dtk doctor` know about MCP. Checked with real Claude Code and a real (small) MCP server: 6,189 tokens became 320 for rows, 5,442 became 301 for structured output, and the document was untouched; every answer right on Haiku and Sonnet (`scripts/mcp-test.sh`). Run `dtk init --global` once to add the matcher to an older install; `dtk doctor` says so.
- **Text around the data.** Up to six short lines (300 characters each) before or after a table stay word for word, and only the table is replaced. This reads a status line above a JSON array, a title above a CSV, a line of totals below JSON, and the layout of database servers such as Supabase, which put a warning about untrusted data on both sides of the rows (that warning is kept, on purpose). Before, none of these was read.
- **Fixed: a wrong table from one line of text.** A line of text above JSON lines made a loose reader take the first JSON line as the header and split it on spaces, so the columns were called `{"id":`, `0,`, `"name":` and the summary had 299 rows. A header with a quote or a brace is not a name now, and the right reader gets the text.
- Checked by: 6 fixtures with the expected prose and rows, and a rule that none of the 31 rejected fixtures (README, source code, diffs, logs) is found to hold a table; every payload compared with its source rows (10,500 cells, none different); hook tests for Bash and MCP; a timing check (no change on 20 to 35 MB of text or on source code); four end-to-end MCP cases and two wrapped Bash cases on Sonnet and Haiku, right every time with dtk, 87% to 94% fewer tokens.

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
