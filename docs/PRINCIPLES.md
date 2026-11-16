# What dtk is for

dtk is the **data token killer**. It is a Claude Code hook that makes the output of tools smaller for
the model **without making it poorer in information**, and it does this **by itself**, **together
with rtk**.

The goal, in the owner's words: *minimize the tokens whilst maximizing the information, in an
automatic way that works with rtk.* The project is about **data**: tables, data frames, CSV, JSON
and JSON Lines, logs, and pages that are read as content. It is not a general summarizer, and it is
not for source code.

Read this page before you change dtk. The rules below came from real mistakes and real
measurements. Each one has a reason.

## The rules

### 1. Automatic first
The value is in the hook, which replaces a long output on its own. A command that the model must
decide to call (`dtk where`, `dtk html`) is a **fallback** for more detail, and it costs a turn.
Before you add a feature, ask: does it work with no call, and what does it cost in tokens?

*Out of the box* means: one install command registers the hook (the installer scripts run
`dtk init --global`); no configuration is needed; every feature is on by default; an upgrade keeps
working (the hook names a path that survives it; `dtk doctor` says when the matcher is old).
A step that the user must remember is a bug.

### 2. Full integration with rtk
- One chained `PostToolUse` hook: **dtk first, then rtk**. Events dtk does not handle go to rtk
  unchanged, byte for byte.
- dtk sees through rtk's rewrites (`cat f` is `rtk read f`; `uv run ...` is `rtk uv run ...`), for
  its own checks and for the user's `exclude_commands`.
- When rtk **cut** an output, dtk reads rtk's saved copy (the tee file, or `rtk recall --full`) and
  serves the whole. It reads only a complete copy that is inside rtk's own `rtk/tee` folder.
- `rtk init`, `rtk init --uninstall` and rtk's `rtk:posthook` pipeline keep working in every order.
  dtk never fights them.
- Test against the **real rtk binary**, not only a model of it. Most of the integration bugs were
  found only that way.

### 3. More information per token, and never a silent loss
- **Lossless when it can be:** a table of 60 rows or fewer is shown as CSV, nothing dropped; the
  text of a web page keeps every visible word.
- **Otherwise say exactly what is not shown** and where all of it is (a short id in dtk's store,
  the saved file, the drill-down command). A summary must not claim more than it covers.
- **Refuse rather than guess.** A table with a column under the wrong name is worse than no table.
  Lines that no reader used are counted (`accounted_for`); too many means the text is not a table.
- A **silent drop is a bug**, even when the output looks tidy. Audit real output for it
  (`examples/audit.rs`, `dtk discover`).
- **Whatever a summary says is reachable must be reachable.** "The full data is on disk" needs a
  path, and the file must exist. Until 0.18.0 a table of 60 rows or fewer that got the statistics
  summary said this and saved nothing. Test the claim: read the path the output gives.

### 4. No model inside dtk
Every view is made by counting and parsing: instant, free, and the same every time. A model in the
loop adds cost, delay and error. It was tried: a message that suggested a smaller model was never
taken in 16 sessions and cost about 7% more. A deterministic digest of the same output is better.

### 5. Evidence, not opinion
- **Measure with real Claude Code** (Sonnet and Haiku, with and without dtk): cost, turns, and
  whether the answer is right. Scripts: `scripts/ab-test.sh`, `mcp-test.sh`, `gh-test.sh`,
  `tools-test.sh`.
- **Check against something independent:** the pandas frame that was printed, `awk`/`jaq`/`qsv`, a
  second HTML parser. Not against dtk's own code.
- **Look at real history** before and after a change (`dtk discover`, `examples/audit.rs`,
  `examples/digest_view.rs`). The best bugs were found this way.
- **Ask the model what the data is, not how it was printed.** A question that says "printed in two
  blocks of columns" makes the model distrust a summary that contradicts it, and the test then
  measures that, not dtk. A weaker model (Haiku) may also offer to read a file instead of reading
  it; run a stronger one (Sonnet) before calling a result a defect.
- **Measured and rejected, so that nobody tries it again without a harder test:** a table shown
  whole as Markdown instead of CSV (`scripts/format-test.sh`: 7 questions, 3 runs, Haiku, 40 rows).
  Both forms were right 21 of 21 times and Markdown cost 27% more tokens, so CSV stays. The test
  was at its ceiling: a harder question or a bigger table could separate the forms, and then the
  script is the place to start. A smaller-model message was rejected the same way (see rule 4).
- **Compare old and new builds over real data before a release:** every big output of the history
  and every text file in the projects. The release must change only what it is meant to change.
- **Report weak spots honestly.** dtk helps the data-heavy minority of tool output. Say what it
  does not help, and what a measurement showed against an idea. Never oversell.

### 5b. Safety: leave alone what was asked for exactly
dtk does nothing when the model asked for exact lines (`head`, `tail`, `sed -n`, a range, `DTK_RAW`,
a `jq` query), when a command prints content (`cat`, `grep`, `find`, `git diff`), for source code,
documents and lists, for an MCP tool that reads files, for a user's `exclude_commands`, and for
dtk's own drill-down commands. Any error means the output passes through (fail open).
A new reader must pass the negative corpus: **no file in this repository that is not data may be
taken for data** (`tests/digest.rs`, `tests/corpus.rs`).

### 5c. Any agent, and no change to what an agent's rules read
dtk is for any agent harness, not only Claude Code. One decision function (`hook::handle`) serves all
of them; each agent gets a thin translation of its event and its reply (`src/harness.rs`), and
nothing else. **Do not rewrite a command** to put dtk in front of it: the text of the command is what
an agent's allow and deny rules read, and a rewrite can stop a deny rule from matching. Replace the
result after the command ran. Where an agent cannot do that, give a rule or `dtk run`, and say that it
is not automatic. **Say what was tried with the real program** and what was written from
documentation; never claim more than the tests show. Every adapter fails open and exits with 0.
**One command connects all of them.** `dtk init --global` registers dtk with Claude Code and every
other supported agent that it finds on the machine, as `rtk init -g` does for its agents, and
`dtk init --uninstall` takes dtk out of all of them. It keeps every setting that the files already
hold, makes a backup once, changes nothing when run again, and one agent that it cannot change does
not stop the others. It works beside rtk in each agent, and that is tried with the real rtk.

### 6. One small program
Pure Rust, one binary, few dependencies (`serde_json`, `csv`, `sha2`, `toml`), Rust 1.80 and newer.
It runs on macOS, Linux and Windows (CI), and installs by script, Homebrew, Scoop, winget, Cargo,
`.deb` and `.rpm`. A release is not done until `install-test.yml` passes for every route.

## How a change is made
1. Find the problem in **evidence** (real output, real history, a real model run).
2. Write the test first or with it: real fixtures, facts counted independently, and a negative case.
3. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` before a commit.
4. Branch, pull request, CI on three systems, squash merge, tag `vX.Y.Z`.
5. Check the release has 19 assets and the crate is on crates.io; run the install test; the tap and
   the bucket follow. `gh run rerun --failed` fixes a macOS runner that GitHub did not provide.
6. Write the CHANGELOG with the measured result, not the hope.

## What is where
- `src/hook.rs` decides (`evaluate`) and answers for Bash, Read and MCP events.
- `src/table.rs`, `foreign.rs`, `logs.rs`, `textcols.rs`, `multiindex.rs`, `ghcli.rs`: readers.
- `src/lean.rs`: the summary of a table. `src/outline.rs`: JSON documents. `src/digest.rs`: logs.
  `src/html.rs`: web pages. `src/query.rs`: the drill-down commands.
- `src/rtk.rs`: everything about rtk. `src/init.rs`, `doctor.rs`: setup.
- `tests/corpus`: real outputs of real tools and what must be made of each.
- `tests/claims.rs`: runs the real binary on every fixture and checks each promise a summary makes
  (the file exists, the counts are the file's, the commands answer). `scripts/compare-builds.sh`
  compares two builds over real files before a release.
