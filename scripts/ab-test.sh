#!/bin/bash
# Gate G6: real Claude Code sessions with and without dtk, on every format.
#
# For each fixture, Claude is asked a question about a file it must `cat`. The answer is also
# computed by another tool (jaq, qsv, awk, grep), so "correct" does not depend on dtk.
# The run "without" sets DTK_DISABLED=1; both runs have the same hooks otherwise (rtk included).
#
# The hooks come from a throwaway project (--setting-sources project), so your own settings are
# not used and not changed. The project registers the dtk binary under test, chained to rtk as
# `dtk init --global` does, and rtk's PreToolUse hook when rtk is installed.
#
# It reports, per case and per arm: the tokens that tool results put in front of the model over
# the whole conversation (not just the first result: a follow-up command to read what the
# summary left out costs tokens too), the number of turns, and whether the answer is right.
#
# Needs: the Claude Code CLI (CLAUDE_BIN), jaq, qsv, awk, grep.
# Environment: DTK_BIN (default target/release/dtk), MODEL (default haiku), AB_RUNS (default 1),
# AB_ONLY (a regex: run only the cases whose name matches), AB_VERBOSE=1 (print the answers).
# Usage: scripts/ab-test.sh            (about 25 seconds per run)
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${CLAUDE_BIN:-}" ]; then
  # Claude Code 2.1.37 ignores the hook's replacement of a tool result; use a newer binary.
  CLAUDE_BIN="$(ls -d "$HOME"/.vscode/extensions/anthropic.claude-code-*/resources/native-binary/claude 2>/dev/null | sort -V | tail -1)"
  CLAUDE_BIN="${CLAUDE_BIN:-claude}"
fi
DTK_BIN="${DTK_BIN:-$HERE/target/release/dtk}"
MODEL="${MODEL:-haiku}"
RUNS="${AB_RUNS:-1}"
C="$HERE/tests/corpus/positive"
D="$HERE/tests/corpus/documents"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
[ -x "$DTK_BIN" ] || { echo "DTK_BIN not found: $DTK_BIN (run cargo build --release)"; exit 2; }

# The project: hooks for the binary under test.
mkdir -p "$WORK/proj/.claude"
PRE=""
if command -v rtk >/dev/null 2>&1; then
  PRE='"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}],'
  CHAIN="$DTK_BIN hook claude --then rtk hook claude"
else
  CHAIN="$DTK_BIN hook claude"
fi
cat > "$WORK/proj/.claude/settings.json" <<EOF
{"hooks":{$PRE"PostToolUse":[{"matcher":"Bash|Read","hooks":[{"type":"command","command":"$CHAIN"}]}]}}
EOF

# ---- the questions and their true answers (computed without dtk) -------------------------------
truth_csv=$(qsv frequency -s region -l 1 "$C/csv_orders.csv" | sed -n 2p | awk -F, '{print $2" "$3}')
truth_jsonl=$(jaq -s '[.[].latency_ms] | max' "$C/jsonl_orders.jsonl")
truth_logfmt=$(grep -c ' level=error ' "$C/logfmt_service.log")
truth_access=$(awk -F'"' '{split($3,a," "); if (a[1]==404) n++} END{print n}' "$C/access_combined.log")
truth_wrapped=$(jaq '[.items[] | select(.phase=="Failed")] | length' "$C/wrapped_items.json")
# The fixture has 80 lines, which is under dtk's minimum size on purpose; use a bigger log.
for _ in 1 2 3 4; do cat "$C/docker_compose_logs.txt"; done > "$WORK/compose_logs.txt"
truth_prefixed=$(grep -c '^web-1' "$WORK/compose_logs.txt")
# Which row holds the largest value? Needs the row, not only the maximum.
truth_slowest=$(qsv sort -s latency_ms -N -R "$C/csv_orders.csv" | qsv select order_id | sed -n 2p)
truth_ps=$(awk 'NR>1 && $3+0>m {m=$3+0; p=$2} END{print p}' "$C/ps_aux.txt")
truth_ci=$(grep -B1 '##\[error\]' "$C/gha_job_long.log" | head -1 | sed 's/^[^ ]* //; s/\r$//')
truth_meta=$(jaq '.packages | length' "$D/cargo_metadata.json")
# Real-world columns, a combined filter that needs the drill-down, and a wide table.
M="$C/messy_orders.csv"
truth_age=$(qsv fmt --out-delimiter '\t' "$M" | awk -F'\t' 'NR>1 && $4 !~ /^(N\/A|NA|-|\?)$/ {s+=$4; n++} END {printf "%d", s/n + 0.5}')
truth_filter=$(qsv fmt --out-delimiter '\t' "$M" | awk -F'\t' 'NR>1 && $2=="eu" {v=$5; gsub(/[$,]/, "", v); if (v+0 > 400) n++} END {print n+0}')
# Shapes that pandas prints: a Python list of records, and to_json(orient="split").
truth_repr=$(qsv slice -l 100 "$C/pandas_csv_index.csv" | qsv sort -s amount -N -R | qsv select order_id | sed -n 2p)
truth_split=$(jaq '[.data[] | select(.[7] == false)] | length' "$C/pandas_split.json")
truth_group=$(qsv fmt --out-delimiter '\t' "$M" | awk -F'\t' 'NR>1 && $2=="eu" {s+=$3; n++} END {printf "%.1f", s/n}')
truth_wide=$(awk -F, 'NR==1 {for (i=1;i<=NF;i++) h[i]=$i; next} {for (i=2;i<=61;i++) s[i]+=$i} END {for (i=2;i<=61;i++) if (s[i] > m) {m=s[i]; b=h[i]}; print b}' "$C/wide_features.csv")

# name | file | question | expected words (all must appear in the answer)
CASES=(
"csv|$C/csv_orders.csv|Which region has the most orders, and how many orders does it have?|$truth_csv"
"jsonl|$C/jsonl_orders.jsonl|What is the highest latency_ms value?|$truth_jsonl"
"logfmt|$C/logfmt_service.log|How many lines have level=error?|$truth_logfmt"
"access log|$C/access_combined.log|How many requests returned status 404?|$truth_access"
"wrapped json|$C/wrapped_items.json|How many pods have phase Failed?|$truth_wrapped"
"prefixed jsonl|$WORK/compose_logs.txt|How many log lines come from web-1?|$truth_prefixed"
"slowest row|$C/csv_orders.csv|Which order_id has the highest latency_ms? Give the order_id.|$truth_slowest"
"ps aux|$C/ps_aux.txt|Which PID uses the most CPU (column %CPU)? Give the PID.|$truth_ps"
"ci log|$C/gha_job_long.log|This is a CI log. What is the output line just before the line with ##[error]?|$truth_ci"
"json document|$D/cargo_metadata.json|How many entries does the packages array have? Give the number.|$truth_meta"
"messy mean|$M|What is the mean of the age column, ignoring the N/A, NA, - and ? values? Round to a whole number.|$truth_age"
"filter+count|$M|How many orders have region eu and a price above 400 dollars? Give the number.|$truth_filter"
"wide means|$C/wide_features.csv|Which of the columns feat_0 to feat_59 has the highest mean? Give its name.|$truth_wide"
"python repr|$C/python_repr_records.txt|This is a Python list of records. Which order_id has the largest amount? Give the order_id.|$truth_repr"
"group mean|$M|What is the mean amount for region eu? Round to one decimal place.|$truth_group"
"cadence|$C/sensor_readings.csv|How many minutes pass between readings, and how many hours long is the longest gap in the data? Give both numbers.|5 3"
"split json|$C/pandas_split.json|This is a pandas DataFrame as JSON (columns, then data rows). How many rows have ok equal to false? Give the number.|$truth_split"
)

run_one() { # question file disabled -> prints "tokens|turns|first_tokens|answer"
  local q="$1" file="$2" off="$3" out="$WORK/run.$$.jsonl"
  ( cd "$WORK/proj" && DTK_DISABLED="$off" "$CLAUDE_BIN" -p "Run this bash command: cat $file   Then answer in one short line: $q" \
      --model "$MODEL" --allowedTools Bash --setting-sources project \
      --output-format stream-json --verbose < /dev/null > "$out" 2>/dev/null )
  if ! grep -q '"type":"result"' "$out" 2>/dev/null; then
    ( cd "$WORK/proj" && DTK_DISABLED="$off" "$CLAUDE_BIN" -p "Run this bash command: cat $file   Then answer in one short line: $q" \
        --model "$MODEL" --allowedTools Bash --setting-sources project \
        --output-format stream-json --verbose < /dev/null > "$out" 2>/dev/null )
  fi
  local lens total first turns answer
  lens=$(jaq -r 'select(.type=="user") | .message.content? | select(type=="array") | .[] | select(.type=="tool_result") | .content | (if type=="array" then map(.text // "") | join("") else . end) | length' "$out" 2>/dev/null)
  total=$(echo "$lens" | awk '{s+=$1} END{print s+0}')
  first=$(echo "$lens" | head -1)
  turns=$(jaq -r 'select(.type=="result") | .num_turns' "$out" 2>/dev/null | tail -1)
  # the whole final message: the answer may be its first paragraph, with a note after it
  answer=$(jaq -r 'select(.type=="result") | .result' "$out" 2>/dev/null | tr '\n' ' ')
  echo "$((total / 4))|${turns:-?}|$(( ${first:-0} / 4 ))|$answer"
}

correct() { # answer expected -> yes/no (every expected word appears)
  local a w
  a="$(echo "$1" | tr '[:upper:]' '[:lower:]')"
  for w in $(echo "$2" | tr '[:upper:]' '[:lower:]'); do case "$a" in *"$w"*) ;; *) echo no; return;; esac; done
  echo yes
}

printf 'model %s, %s run(s) per arm; dtk: %s\n' "$MODEL" "$RUNS" "$("$DTK_BIN" --version)"
printf '%-14s %9s %9s %6s  %-9s %-9s  %s\n' "case" "tok w/o" "tok with" "saved" "right w/o" "right with" "turns w/o -> with   (expected)"
fail=0
for c in "${CASES[@]}"; do
  IFS='|' read -r name file q want <<< "$c"
  [ -z "${AB_ONLY:-}" ] || [[ "$name" =~ $AB_ONLY ]] || continue
  t0=0; t1=0; n0=0; n1=0; r0=0; r1=0; ok0=0; ok1=0
  for _ in $(seq "$RUNS"); do
    IFS='|' read -r tok0 turn0 _ a0 <<< "$(run_one "$q" "$file" 1)"
    IFS='|' read -r tok1 turn1 _ a1 <<< "$(run_one "$q" "$file" 0)"
    t0=$((t0 + tok0)); t1=$((t1 + tok1)); n0=$((n0 + ${turn0:-0})); n1=$((n1 + ${turn1:-0}))
    [ -z "${AB_VERBOSE:-}" ] || printf '   without: %s\n   with:    %s\n' "$a0" "$a1"
    [ "$(correct "$a0" "$want")" = yes ] && ok0=$((ok0 + 1))
    [ "$(correct "$a1" "$want")" = yes ] && ok1=$((ok1 + 1))
  done
  t0=$((t0 / RUNS)); t1=$((t1 / RUNS))
  if [ "$t0" -gt 0 ]; then saved=$(( (t0 - t1) * 100 / t0 )); else saved=0; fi
  printf '%-14s %9s %9s %5s%%  %-9s %-9s  %s -> %s   (%s)\n' "$name" "$t0" "$t1" "$saved" "$ok0/$RUNS" "$ok1/$RUNS" "$n0" "$n1" "$want"
  [ "$ok1" -ge "$ok0" ] || fail=1          # never less correct than without dtk
  [ "$ok1" -ge 1 ] || fail=1               # right at least once with dtk
  # Never more tokens over the conversation, give or take 15% and 300 tokens (about half of
  # Claude Code's preview). When Claude Code has saved a big output to a file, the model already
  # sees a preview of about 550 tokens, and dtk swaps it for a summary of about the same size
  # that covers all rows; a drill-down command's output may add a few hundred more. That costs
  # almost nothing and tells more.
  [ "$t1" -le $(( t0 * 115 / 100 + 300 )) ] || fail=1
done
[ "$fail" = 0 ] && echo "G6 PASS: with dtk, answers are as right as without, and no case costs noticeably more tokens" || { echo "G6 FAIL"; exit 1; }
