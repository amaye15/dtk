# Changelog

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
