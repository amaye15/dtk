#!/bin/bash
# Real Claude Code asked about `gh` output, with and without dtk.
#
# `gh` is a shim on the PATH that prints real output kept in tests/corpus/gh (a list of CI runs,
# and the log of a failed run), so the test needs no network and no login. The hook sees the
# command `gh run list ...` as it would in a real session. The true answers come from awk and
# grep, not from dtk.
#
# Environment: DTK_BIN (default target/release/dtk), MODEL (default haiku), AB_RUNS (default 1),
# AB_VERBOSE=1 (print the answers), CLAUDE_BIN.
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${CLAUDE_BIN:-}" ]; then
  CLAUDE_BIN="$(ls -d "$HOME"/.vscode/extensions/anthropic.claude-code-*/resources/native-binary/claude 2>/dev/null | sort -V | tail -1)"
  CLAUDE_BIN="${CLAUDE_BIN:-claude}"
fi
DTK_BIN="${DTK_BIN:-$HERE/target/release/dtk}"
MODEL="${MODEL:-haiku}"
RUNS="${AB_RUNS:-1}"
FIX="$HERE/tests/corpus/gh"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
[ -x "$DTK_BIN" ] || { echo "DTK_BIN not found: $DTK_BIN (run cargo build --release)"; exit 2; }

mkdir -p "$WORK/proj/.claude" "$WORK/bin"
cat > "$WORK/proj/.claude/settings.json" <<EOT
{"hooks":{"PostToolUse":[{"matcher":"Bash|Read|mcp__.*","hooks":[{"type":"command","command":"$DTK_BIN hook claude"}]}]}}
EOT
cat > "$WORK/bin/gh" <<EOT
#!/bin/sh
case "\$*" in
  *"run list"*) cat "$FIX/run_list.tsv" ;;
  *"run view"*"--log-failed"*) cat "$FIX/run_log_failed.tsv" ;;
  *) echo "gh shim: no output for: \$*" >&2; exit 1 ;;
esac
EOT
chmod +x "$WORK/bin/gh"

truth_runs=$(awk -F'\t' '$4=="install test"' "$FIX/run_list.tsv" | wc -l | tr -d ' ')
truth_failed=$(grep -o 'test [a-z_]* \.\.\. FAILED' "$FIX/run_log_failed.tsv" | head -1 | awk '{print $2}')

# name | command | question | expected words
CASES=(
"gh run list|gh run list -R amaye15/dtk --limit 100|How many runs in the output are of the workflow named install test? Give the number.|$truth_runs"
"gh run log|gh run view 41587544980 -R amaye15/dtk --log-failed|This is the log of a failed CI run. Which test failed? Give the test name.|$truth_failed"
)

run_one() { # command question disabled -> tokens|turns|answer
  local cmd="$1" q="$2" off="$3" out="$WORK/run.$$.jsonl"
  local prompt="Run this bash command: $cmd   Then answer in one short line: $q"
  for _ in 1 2; do
    ( cd "$WORK/proj" && PATH="$WORK/bin:$PATH" DTK_DISABLED="$off" "$CLAUDE_BIN" -p "$prompt" --model "$MODEL" \
        --allowedTools Bash --setting-sources project --output-format stream-json --verbose < /dev/null > "$out" 2>/dev/null )
    grep -q '"type":"result"' "$out" 2>/dev/null && break
  done
  local lens total turns answer
  lens=$(jaq -r 'select(.type=="user") | .message.content? | select(type=="array") | .[] | select(.type=="tool_result") | .content | (if type=="array" then map(.text // "") | join("") else . end) | length' "$out" 2>/dev/null)
  total=$(echo "$lens" | awk '{s+=$1} END{print s+0}')
  turns=$(jaq -r 'select(.type=="result") | .num_turns' "$out" 2>/dev/null | tail -1)
  answer=$(jaq -r 'select(.type=="result") | .result' "$out" 2>/dev/null | tr '\n' ' ')
  echo "$((total / 4))|${turns:-?}|$answer"
}

correct() {
  local a w; a="$(echo "$1" | tr '[:upper:]' '[:lower:]')"
  for w in $(echo "$2" | tr '[:upper:]' '[:lower:]'); do case "$a" in *"$w"*) ;; *) echo no; return;; esac; done
  echo yes
}

printf 'model %s, %s run(s) per arm; dtk: %s\n' "$MODEL" "$RUNS" "$("$DTK_BIN" --version)"
printf '%-14s %9s %9s %6s  %-9s %-10s  %s\n' "case" "tok w/o" "tok with" "saved" "right w/o" "right with" "turns w/o -> with   (expected)"
fail=0
for c in "${CASES[@]}"; do
  IFS='|' read -r name cmd q want <<< "$c"
  t0=0; t1=0; n0=0; n1=0; ok0=0; ok1=0
  for _ in $(seq "$RUNS"); do
    IFS='|' read -r tok0 turn0 a0 <<< "$(run_one "$cmd" "$q" 1)"
    IFS='|' read -r tok1 turn1 a1 <<< "$(run_one "$cmd" "$q" 0)"
    t0=$((t0 + tok0)); t1=$((t1 + tok1)); n0=$((n0 + ${turn0:-0})); n1=$((n1 + ${turn1:-0}))
    [ -z "${AB_VERBOSE:-}" ] || printf '   without: %s\n   with:    %s\n' "$a0" "$a1"
    [ "$(correct "$a0" "$want")" = yes ] && ok0=$((ok0 + 1))
    [ "$(correct "$a1" "$want")" = yes ] && ok1=$((ok1 + 1))
  done
  t0=$((t0 / RUNS)); t1=$((t1 / RUNS))
  if [ "$t0" -gt 0 ]; then saved=$(( (t0 - t1) * 100 / t0 )); else saved=0; fi
  printf '%-14s %9s %9s %5s%%  %-9s %-10s  %s -> %s   (%s)\n' "$name" "$t0" "$t1" "$saved" "$ok0/$RUNS" "$ok1/$RUNS" "$n0" "$n1" "$want"
  [ "$ok1" -ge "$ok0" ] || fail=1
  [ "$ok1" -ge 1 ] || fail=1
  [ "$t1" -le $(( t0 * 115 / 100 + 300 )) ] || fail=1
done
[ "$fail" = 0 ] && echo "GH PASS: answers are as right as without dtk, and no case costs more tokens" || { echo "GH FAIL"; exit 1; }
