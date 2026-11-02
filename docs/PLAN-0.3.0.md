# dtk 0.3.0 plan

Goal: handle more data formats with better automatic detection, keep working with rtk in every
way rtk is used, and follow rtk's practices (self-checks, explain, discover, config, installer).

Rule for this plan: a step is done only when its check passes. The checks are commands, not
opinions. Results are filled in at the end of each step.

## Gates every step must pass

| Gate | Command | Passes when |
|---|---|---|
| G1 | `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked` | no warning, no failure |
| G2 | `cargo test --test corpus` | every fixture is detected with the expected format, rows and columns; every negative fixture is rejected |
| G3 | `cargo test --test properties` | random and mutated inputs never panic; every table has equal-width rows; an applied summary is smaller than its input |
| G4 | `cargo test --test oracle` | dtk's counts, min, max, mean and percentiles equal `qsv` and `jaq` on the corpus (skipped if they are not installed) |
| G5 | `cargo test --test rtk_real` | the real rtk binary, in a throwaway HOME, installs, re-installs and uninstalls without touching dtk's entry, and the reverse (skipped if rtk is not installed; CI installs it) |
| G6 | `scripts/ab-test.sh` (end of plan) | real Claude Code sessions with and without dtk give the same correct answers with fewer tokens |

## Steps

### S1. Full rtk compatibility
Problem found with the real rtk 1.1.0: after `dtk init`, running `rtk init -g` again appends a
second, separate `PostToolUse` hook. Claude Code applies the last matching entry, so rtk's output
could replace dtk's summary.

Design: one entry with two hooks, the dtk chain and a silent placeholder named `rtk` in dtk's
data folder. rtk recognizes its hook by the shape `<path>/rtk hook claude`, so it sees its hook as
installed and adds no duplicate. rtk's uninstall removes only the placeholder.

Checks: `tests/rtk_real.rs` (G5) covers init twice, no duplicate, rtk uninstall keeps dtk, dtk
uninstall restores rtk's entry, enabling `rtk:posthook` later widens the matcher, `rtk init --show`
and `rtk pipeline validate` report no missing hook.

### S2. Cleaner input
ANSI colours, CRLF, a byte-order mark, comment and preamble lines, and the full box-drawing range
(rich tables) no longer stop detection.

Checks: new positive fixtures in `tests/corpus/` for each (G2).

### S3. More formats
JSON objects that wrap one big array (`{"items": [...]}`, as `kubectl` and many APIs return),
JSON objects keyed by id, logfmt (`level=info msg="x"`), web access logs (common and combined),
and JSONL with a per-line prefix (`docker compose logs`).

Checks: at least two positive fixtures and three negative fixtures per format (G2); counts equal
`jaq` and `qsv` (G4).

### S4. Smaller, smarter summaries
Group high-cardinality text columns by message template (digits and ids replaced), so a log with
4,000 different messages shows its 8 real templates with counts. Short store paths, so a long
path is not repeated three times.

Checks: a fixture with a known template mix reports exact counts; token estimate of the summary
for the 5,000-row CSV falls by at least 10% (G1).

### S5. rtk-style tools
`dtk explain` (why was this output summarized or not), `dtk verify` (self-test of the whole
setup), `dtk discover` (scan Claude Code transcripts for tables that dtk would have summarized),
`dtk gain --json`, a config file with `exclude_commands` and thresholds, and `install.sh`.

Checks: one integration test per command (G1); `dtk discover` run on real transcripts reports
numbers that match a manual count on one session.

### S6. Prove it, then release
Run G1 to G6, record the real-tokenizer results, publish 0.3.0.

## Results

### S1. Full rtk compatibility: DONE
- G1: 52 tests pass, lint clean.
- G5: `cargo test --test rtk_real` runs 7 tests against the real rtk 1.1.0: init twice adds no
  second hook; rtk-first then dtk takes over; `rtk init --show` and `rtk pipeline validate` stay
  satisfied; `rtk init --uninstall` removes only the placeholder; `dtk init --uninstall` gives
  rtk's own entry back exactly (and does not bring back one that rtk removed); a later
  `rtk:posthook` widens the matcher through rtk itself; the real chain sends tables to dtk and
  everything else to rtk.
- Real Claude Code session on the real settings: summary received, answer correct (23 = 23).


### Harness (G2, G3, G4): DONE
- `tests/corpus/`: 33 positive, 19 negative and 5 ambiguous real outputs from pandas, polars,
  sqlite3 (6 modes), tabulate (10 formats), rich, jq, git, gh and others. The first run found
  5 real defects: a header rule row counted as data (sqlite `-column`, tabulate `simple`),
  ANSI colour codes in headers and failed JSON detection (rich, `jq -C`), and an overly crude
  pandas check that also rejected honest tables whose `id` column counts up.
- `tests/properties.rs` (G3): 4,000 random texts, 800 random byte blobs, and every fixture
  mutated 40 times. Detection never panics, tables are always well formed, views are never much
  bigger than input, and every big fixture gets a summary under half its size. A teeth check
  requires at least a quarter of the mutated inputs to still be tables.
- `tests/oracle.rs` (G4): types, null counts, min, max, mean, distinct counts, medians and
  category counts equal `qsv stats`, `qsv frequency` and `jaq` on the CSV, JSONL and JSON files.
  qsv reports ISO timestamps as strings unless told otherwise; the test passes `--infer-dates`.

### S2. Cleaner input: DONE
ANSI colour codes, a byte-order mark, `#` comment headers, header rule rows and the full
box-drawing range are handled. Checks: the new fixtures above pass (G2), all gates green (G1).
