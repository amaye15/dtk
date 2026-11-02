#!/bin/bash
# Gate G6: real Claude Code sessions with and without dtk, on every format.
#
# For each fixture, Claude is asked a question about a file it must `cat`. The answer is also
# computed by another tool (jaq, qsv, awk, grep), so "correct" does not depend on dtk.
# The run "without" sets DTK_DISABLED=1; both runs have the same hooks otherwise (rtk included).
#
# Needs: the Claude Code CLI (CLAUDE_BIN, default `claude`), jaq, qsv, awk, grep.
# Usage: scripts/ab-test.sh            (about 20 seconds per run, 2 runs per format)
set -u
CLAUDE_BIN="${CLAUDE_BIN:-claude}"
MODEL="${MODEL:-haiku}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
C="$HERE/tests/corpus/positive"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# name | file | question | expected words (all must appear in the answer)
truth_csv=$(qsv frequency -s region -l 1 "$C/csv_orders.csv" | sed -n 2p | awk -F, '{print $2" "$3}')
truth_jsonl=$(jaq -s '[.[].latency_ms] | max' "$C/jsonl_orders.jsonl")
truth_logfmt=$(grep -c ' level=error ' "$C/logfmt_service.log")
truth_access=$(awk -F'"' '{split($3,a," "); if (a[1]==404) n++} END{print n}' "$C/access_combined.log")
truth_wrapped=$(jaq '[.items[] | select(.phase=="Failed")] | length' "$C/wrapped_items.json")
# The fixture has 80 lines, which is under dtk's minimum size on purpose; use a bigger log.
for _ in 1 2 3 4; do cat "$C/docker_compose_logs.txt"; done > "$WORK/compose_logs.txt"
truth_prefixed=$(grep -c '^web-1' "$WORK/compose_logs.txt")

CASES=(
"csv|$C/csv_orders.csv|Which region has the most orders, and how many orders does it have?|$truth_csv"
"jsonl|$C/jsonl_orders.jsonl|What is the highest latency_ms value?|$truth_jsonl"
"logfmt|$C/logfmt_service.log|How many lines have level=error?|$truth_logfmt"
"access log|$C/access_combined.log|How many requests returned status 404?|$truth_access"
"wrapped json|$C/wrapped_items.json|How many pods have phase Failed?|$truth_wrapped"
"prefixed jsonl|$WORK/compose_logs.txt|How many log lines come from web-1?|$truth_prefixed"
)

run_one() { # dir question file disabled -> prints "chars turns answer"
  local dir="$1" q="$2" file="$3" off="$4" out="$WORK/run.$$.jsonl"
  ( cd "$dir" && DTK_DISABLED="$off" "$CLAUDE_BIN" -p "Run this bash command: cat $file   Then answer in one short line: $q" \
      --model "$MODEL" --allowedTools Bash --output-format stream-json --verbose < /dev/null > "$out" 2>/dev/null )
  local chars turns answer
  chars=$(jaq -c 'select(.type=="user") | .message.content? | select(type=="array") | .[] | select(.type=="tool_result") | .content | if type=="array" then map(.text) | join("") else . end | length' "$out" 2>/dev/null | head -1)
  turns=$(jaq -r 'select(.type=="result") | .num_turns' "$out" 2>/dev/null | tail -1)
  answer=$(jaq -r 'select(.type=="result") | .result' "$out" 2>/dev/null | tail -1 | tr '\n' ' ')
  echo "${chars:-0}|${turns:-?}|$answer"
}

correct() { # answer expected -> yes/no (every expected word appears)
  local a="$1" w
  for w in $2; do case "$a" in *"$w"*) ;; *) echo no; return;; esac; done
  echo yes
}

mkdir -p "$WORK/proj"
printf '%-16s %12s %12s %8s   %s\n' "format" "tokens w/o" "tokens with" "saved" "answers (without / with)"
fail=0
for c in "${CASES[@]}"; do
  IFS='|' read -r name file q want <<< "$c"
  IFS='|' read -r c0 t0 a0 <<< "$(run_one "$WORK/proj" "$q" "$file" 1)"
  IFS='|' read -r c1 t1 a1 <<< "$(run_one "$WORK/proj" "$q" "$file" 0)"
  ok0=$(correct "$a0" "$want"); ok1=$(correct "$a1" "$want")
  tok0=$((c0 / 4)); tok1=$((c1 / 4))
  saved=$(( tok0 > 0 ? (tok0 - tok1) * 100 / tok0 : 0 ))
  printf '%-16s %12s %12s %7s%%   %s / %s   (expected: %s; turns %s -> %s)\n' "$name" "$tok0" "$tok1" "$saved" "$ok0" "$ok1" "$want" "$t0" "$t1"
  [ "$ok1" = yes ] || fail=1
  [ "$tok1" -lt "$tok0" ] || fail=1
done
[ "$fail" = 0 ] && echo "G6 PASS: correct answers with dtk, and fewer tokens on every format" || { echo "G6 FAIL"; exit 1; }
