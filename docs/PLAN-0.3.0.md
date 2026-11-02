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

### S3. More formats: DONE
JSON objects that wrap one big array (also two levels down, and objects keyed by id), logfmt,
web access logs (common and combined), and JSONL with a per-line prefix.
- G2: 11 new positive and 10 new negative fixtures, all as expected. The positives include two
  real GitHub API responses (`{"total_count":...,"items":[...]}`, 83 columns per item).
- G4: five new oracle tests against tools that share no code with dtk: wrapped and keyed JSON
  against `jaq`; logfmt line counts and level counts against `grep`, mean latency against `awk`;
  access-log status counts and mean bytes against `awk`; prefixed JSONL against `sed` and `jaq`.
- Real defects the new fixtures found, now fixed: a wide table with few rows was shown in full
  (83 columns of nested JSON, no saving); a tiny table could get a summary bigger than itself;
  a `-` in the bytes column made it text instead of a number.

### S4. Smaller, smarter summaries: DONE
- Text columns with many distinct values are grouped by message shape (digits become `#`, long
  hex ids `<hex>`, uuids `<uuid>`). Unit test with a known mix (2,000 / 700 / 300 over 3,000
  distinct messages) checks the exact counts. On a 4,000-line logfmt log whose messages all
  differ, the `msg` column reads as 6 templates with counts, and the whole view is 1,343 real
  tokens against 150,409 for the raw log (99.1% less).
- A long saved-file path (Claude Code keeps big outputs deep under `~/.claude/projects/...`) is
  given a short symlink in dtk's store, so it is not repeated in three lines. Real tokenizer
  (o200k_base), 5,000-row CSV saved under a realistic long path: 996 tokens before, 792 after
  (-20.5%; target was -10%). The test checks that the link reads through to the saved file.
- A test with deliberately mixed data found the limit: a random hex id that happens to be all
  digits is a number, so a template can split in two (6 and 3 rows in the log above).

### S5. rtk-style tools: DONE
- One decision function, `evaluate`, now serves the hook, `dtk explain`, `dtk verify` and
  `dtk discover`. G1 test `explain_agrees_with_the_hook_on_every_corpus_fixture` runs all 77
  corpus fixtures through both and requires that the hook replaces the output exactly when
  `explain` says APPLY.
- `dtk verify`: 24 built-in PASS/FAIL checks (each format detected and summarized to under a
  third, prose and code left alone, row requests and exclusions honoured, small outputs left
  alone, saved-to-file output replaced), then the setup checks of `dtk doctor`. It first failed
  on a healthy install because three of its own samples were under the 1,200-token minimum.
- `dtk discover` on a hand-built transcript is checked for exact counts (6 results, 3 tables, 2
  shown in full, 1 saved to a file, a ranged Read skipped, a head request skipped). That test
  found that short transcript lines were skipped, which dropped every saved-to-file output.
  On one real session (2,150 tool results) the result count equals an independent `jaq` count.
  On the real 30-day history (84 sessions, 53,858 results, 1.8 s): 58 tables, 11 replaceable,
  about 17K estimated tokens. Most tables in that history are small, and the biggest
  opportunities are test files from building dtk, so dtk is not a big saver for ordinary
  coding work.
- `~/.config/dtk/config.toml` (`exclude_commands`, thresholds). A broken file falls back to the
  defaults, and `dtk doctor` says so (tested).
- `dtk gain --json`, and `install.sh`: tested against the real v0.2.0 release (checksum ok,
  installed, runs); a deliberately corrupted download is refused and nothing is installed.
- CI installs the real rtk, qsv (macOS) and jaq, so G4 and G5 run there instead of being skipped.

### S6. Prove it, then release: DONE
- G1: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test`:
  94 tests across 10 suites, all passing, on this Mac and in CI (macOS and Linux).
- G2 corpus, G3 properties, G4 oracle, G5 real rtk: pass locally. In CI the real rtk is
  installed, so G5 runs there too. Running it on Linux found that rtk locates its config through
  `XDG_CONFIG_HOME` before `HOME`, so the tests now isolate both.
- G6 (`scripts/ab-test.sh`, real Claude Code 2.1.314 on Haiku, one run per cell):

| Format | Tokens without dtk | With dtk | Saved | Answer correct (without / with) |
|---|---:|---:|---:|---|
| csv | 4,536 | 422 | 90% | yes / yes |
| jsonl | 567 (Claude Code's preview of a saved file) | 482 | 14% | yes / yes |
| logfmt | 3,167 | 735 | 76% | yes / yes |
| access log | 3,822 | 715 | 81% | yes / yes |
| wrapped json | 1,735 | 250 | 85% | yes / yes |
| prefixed jsonl | 4,497 | 438 | 90% | yes / yes |

  In an earlier run of the same script the model answered the access-log question wrongly
  without dtk and correctly with it; that is model variance, not a result.
- Honest limit from `dtk discover`: on this machine's own 30-day history only 11 outputs would
  have been replaced (about 17K estimated tokens), because ordinary coding work rarely prints big
  tables. dtk is worth most for data-heavy work.
