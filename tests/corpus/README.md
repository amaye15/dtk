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
- `polars_orders.txt`: polars 2.1 `print(df)` with the row limit lifted
- `sqlite_*`: `sqlite3 -header -box|-table|-markdown|-column|-csv|-json` (SQLite 3.50)
- `tabulate_*`: tabulate 0.10, ten table formats
- `rich_table*.txt`: rich, plain and with ANSI colour codes
- `json_array_ansi.json`: `jq -C .` (ANSI colours)
- `git_log_tsv.txt`: `git log --pretty=format:'%h%x09%an%x09%ad%x09%s'` on a real repository
- `csv_comment_header.csv`, `csv_bom.csv`: a `#` comment header, and a byte-order mark
- `negative/`: this repository's README, source and changelog; real `cargo test`, `git diff`,
  `git log`, `env`, `brew list`, `ls -l`; a Python traceback; a shell script; YAML; prose
- `either/`: `ps aux`, `df -h`, `lsof`, `netstat`, `qsv stats | qsv table`
