# Detection corpus

Real output of real tools, plus what dtk must make of it (`manifest.json`). `tests/corpus.rs`
checks every entry.

| Folder | Meaning |
|---|---|
| `positive/` | must be detected with the listed format, rows, columns and headers |
| `negative/` | must be rejected (a false positive replaces real output with a summary) |
| `either/` | real output where both answers are fine, but a detected table must be well formed |

How the files were made (tool and version on 2026-11-02):

- `csv_*`, `tsv_*`, `jsonl_*`, `json_array_*`, `pandas_*`: pandas 3.0 on seeded random orders data
- `pandas_groupby_*.txt`, `pandas_series_multiindex.txt`, `pandas_pivot_multiindex.txt`, `pandas_groupby_single_key_two_level_header.txt`, `pandas_set_index_named.txt`: pandas 3.0 `groupby().agg()`, a Series with a MultiIndex, and `pivot_table` with MultiIndex rows and columns, printed with `to_string()` or `print()` (a repeated index label is left blank); the expected rows and headers come from the pandas frames, not from dtk
- `pandas_to_dict*.txt`, `pandas_values_tolist.txt`, `pandas_dict_per_line.txt`, `pandas_to_json_columns.json`: pandas 3.0 `to_dict()` (a dict per column), `to_dict("index")`, `to_dict("split")`, `values.tolist()`, one `to_dict("records")` dict per line, and `to_json()`; `pandas_to_string_datetime_spaces.txt` and `pandas_datetime_to_string.txt` are `to_string()` of frames with a datetime column (and labels with spaces); `negative/pandas_datetime_*.txt` are layouts that are refused on purpose (a DatetimeIndex, and `to_string(index=False)` with a datetime first column)
- `wrapped/*`: synthetic orders with prose around them, as scripts and database MCP servers print them: a status line or a title above a JSON array, pretty JSON, CSV or JSON lines; a line of totals below a JSON array; and a database server's warning text on both sides of its rows (in the layout of Supabase's `execute_sql`); `uv_run_cold_cache.txt` is real: `uv run script.py 2>&1` on a cold cache, 13 lines of uv's download and install progress above `df.to_string()` of a 400-row frame. `manifest.json` has the expected prose (`before`, `after`).
- `gh/*`: output of the GitHub CLI without a terminal (tab-separated, no header). `run_list`, `pr_list`, `pr_checks`, `release_list`, `repo_list` and `run_log_failed` (the log of a failed CI run) are real output for this repository (`gh run list`, `gh pr list`, `gh pr checks`, `gh release list`, `gh repo list`, `gh run view --log-failed`); `issue_list`, `search_issues` and `label_list` have the layout that `gh` printed for other repositories, with made-up rows. `manifest.json` has the command that made each file, because the command names the fields.
- `pages/docs_rs_dtk.html`: a real web page, `curl -sL https://docs.rs/dtk/latest/dtk/` (this crate's own documentation), for the HTML reader.
- `polars_orders.txt`: polars 2.1 `print(df)` with the row limit lifted
- `sqlite_*`: `sqlite3 -header -box|-table|-markdown|-column|-csv|-json` (SQLite 3.50)
- `tabulate_*`: tabulate 0.10, ten table formats
- `rich_table*.txt`: rich, plain and with ANSI colour codes
- `json_array_ansi.json`: `jq -C .` (ANSI colours)
- `git_log_tsv.txt`: `git log --pretty=format:'%h%x09%an%x09%ad%x09%s'` on a real repository
- `gha_job_failed.log`, `gha_job_long.log`: real GitHub Actions job logs of this repository
  (`gh api repos/amaye15/dtk/actions/jobs/<id>/logs`), one with a failed step
- `pandas_csv_index.csv`, `python_repr_records.txt`, `pandas_split.json`, `numpy_matrix.txt`: pandas 3.0 `to_csv()` (with the index column), `str(df.to_dict("records"))`, `to_json(orient="split")`, and numpy `array2string`; `sqlite_list.txt` is `sqlite3 -list`
- `psql_expanded.txt`, `mysql_vertical.txt`: synthetic, in the layout of `psql -x` and MySQL `\G`; `sqlite_line.txt` is `sqlite3 -line`; `pandas_table.html` is `df.to_html()`
- `sensor_readings.csv`: synthetic time series, one row every 5 minutes with a three-hour outage after row 149; `temp_c` and `temp_f` are the same measurement
- `messy_orders.csv`: synthetic orders with what real data has: `N/A`, `-` and `?` in a number column, `$1,234.56`, `45.3%` and `1,234` formatted numbers, `MM/DD/YYYY` dates, two outliers and three repeated rows
- `small_values.csv`: synthetic p-values, rates, losses and learning rates between 1e-8 and 0.1, as in statistics and machine learning (a summary that rounds to four decimals shows them as 0)
- `wide_features.csv`: 60 numeric feature columns, as in a machine-learning data set
- `ps_aux.txt`, `ps_ef.txt`, `df_h.txt`: synthetic, in the layout of macOS `ps aux`, `ps -ef` and `df -h` (a free-text
  last column; the real outputs on the author's machine were checked by hand and are not committed)
- `csv_comment_header.csv`, `csv_bom.csv`: a `#` comment header, and a byte-order mark
- `negative/markdown_page_with_table.md`, `ls_la_listing.txt`, `gh_run_log_tab_separated.tsv`, `tensor_shapes.txt`, `grep_results_with_size_list.txt`: synthetic, in the shape of real outputs that 0.13.0 wrongly read as tables (found by comparing the lines of every big output in a real history with the lines each table accounted for)
- `negative/`: this repository's README, source and changelog; real `cargo test`, `git diff`,
  `git log`, `env`, `brew list`, `ls -l`; a Python traceback; a shell script; YAML; prose
- `either/`: `ps aux`, `df -h`, `lsof`, `netstat`, `qsv stats | qsv table`
