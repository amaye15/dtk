# dtk 0.5.0: plan and proof

Goal: keep more of what a question needs, handle more formats, and never break what rtk does. Each step has a check that fails if the step is wrong.

## What the user's own history said first

A scan of 60 days of Claude Code transcripts (tool results of 1,500 tokens or more, counted as the model saw them):

| Shape | Share of tokens |
|---|---:|
| Source code read with `Read` | 40% |
| Other text (`sed -n` slices, `grep`, `git`) | 36% |
| Prose documents read with `Read` | 13% |
| Timestamped logs (mostly `gh api .../logs`) | 3% |
| Build and test output | 3% |
| Aligned tables | 2% |
| CSV, JSON, JSONL | about 1% |

So: exact-text reads dominate and must never be compressed. A tool for data helps only the data-heavy minority of outputs. Where it helps, it must not lose facts. That sets the order of the work: correctness first, then proof of what a summary keeps, then formats.

## Steps and their gates

| # | Step | Gate (a test that fails if the step is wrong) |
|---|---|---|
| 1 | See through rtk's command rewrites; leave output that rtk cut | `tests/rtk_compat.rs`: the real rtk rewrites `head`, `tail`, `sed -n`, `cat`, `curl`, `jq`; dtk must keep the rows, honour `exclude_commands`, and leave an rtk-truncated document alone |
| 2 | State what a summary must keep | `tests/retention.rs`: row count, column names, ranges, which row holds each extreme, value counts, time span, first and last row, the first errors. Computed with its own arithmetic. 564 of 742 facts before, 100% after |
| 3 | `ps aux`, `ps -ef`, `df -h` | `src/textcols.rs` unit tests, the corpus matrix, and an oracle: every cell of the real `ps aux` (453 rows) equals an independent `split(None, 10)` |
| 4 | Timestamped text logs | unit tests, two real CI logs in the corpus, fuzz tests (found a slice at a non-character boundary) |
| 5 | JSON document outlines | `tests/outline.rs`: names every key, small documents untouched, jq queries and drill-downs untouched, tables still tables |
| 6 | Real model, with and without dtk | `scripts/ab-test.sh`: ten questions, true answers from jaq, qsv, awk and grep |

## Found on the way

- rtk rewrites `head -n 400 f` to `rtk read f --head-lines 400`; dtk summarized the 400 rows that were asked for.
- `exclude_commands = ["^cat "]` stopped working under rtk.
- A table that rtk had cut was summarized as "all rows".
- A JSON object with a small array of objects beside a big object was summarized as the small array; the rest of the document vanished.
- A parser change made the pandas "unnamed index and split datetime" output look like a table with wrong labels. The existing negative corpus caught it.
- A longer summary header made Haiku talk about the tool instead of answering.

## Not done, on purpose

- `ls -l`, `find`, `git ls-files`: in a listing the names are the content.
- `netstat`, `lsof`: column names with spaces cannot be told from separate columns, and a wrong guess puts values under the wrong name.
- Compressing source code or prose: the model needs the exact text.
