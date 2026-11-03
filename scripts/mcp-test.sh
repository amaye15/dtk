#!/bin/bash
# Real Claude Code asked about data that an MCP tool returns, with and without dtk.
#
# scripts/mcp-rows-server.py is a tiny MCP server. The hook is registered the way `dtk init`
# registers it (matcher Bash|Read|mcp__.*). The run "without" sets DTK_DISABLED=1. The true
# answers come from jaq, not from dtk. The third case returns a big JSON document that is not a
# table: dtk must leave it alone (the model asked a server for those values).
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
SERVER="$HERE/scripts/mcp-rows-server.py"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
[ -x "$DTK_BIN" ] || { echo "DTK_BIN not found: $DTK_BIN (run cargo build --release)"; exit 2; }

mkdir -p "$WORK/proj/.claude"
cat > "$WORK/proj/.claude/settings.json" <<EOT
{"hooks":{"PostToolUse":[{"matcher":"Bash|Read|mcp__.*","hooks":[{"type":"command","command":"$DTK_BIN hook claude"}]}]}}
EOT
cat > "$WORK/mcp.json" <<EOT
{"mcpServers":{"big":{"command":"python3","args":["$SERVER"]}}}
EOT
python3 "$SERVER" --dump rows > "$WORK/rows.json"
python3 "$SERVER" --dump doc > "$WORK/doc.json"
truth_max=$(jaq -r 'max_by(.amount) | .id' "$WORK/rows.json")
truth_max_amount=$(jaq -r 'max_by(.amount) | .amount' "$WORK/rows.json")
truth_fail=$(jaq -r '[.[] | select(.status=="fail")] | length' "$WORK/rows.json")
truth_secret=$(jaq -r '.settings.secret_code' "$WORK/doc.json")

# name | tool | question | expected words
CASES=(
"mcp rows|rows|Which id has the highest amount? Give the id.|$truth_max"
"mcp structured|rows_structured|How many rows have status fail? Give the number.|$truth_fail"
"mcp wrapped|rows_wrapped|What is the largest amount, and which id has it? Give both.|$truth_max_amount $truth_max"
"mcp document|doc|What is the value of settings.secret_code? Give the value.|$truth_secret"
)

run_one() { # tool question disabled -> tokens|turns|answer
  local tool="$1" q="$2" off="$3" out="$WORK/run.$$.jsonl"
  local prompt="Call the tool mcp__big__$tool once with no arguments. Then answer in one short line: $q"
  for _ in 1 2; do
    ( cd "$WORK/proj" && DTK_DISABLED="$off" "$CLAUDE_BIN" -p "$prompt" --model "$MODEL" \
        --mcp-config "$WORK/mcp.json" --allowedTools "mcp__big__$tool" --setting-sources project \
        --output-format stream-json --verbose < /dev/null > "$out" 2>/dev/null )
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
printf '%-16s %9s %9s %6s  %-9s %-10s  %s\n' "case" "tok w/o" "tok with" "saved" "right w/o" "right with" "turns w/o -> with   (expected)"
fail=0
for c in "${CASES[@]}"; do
  IFS='|' read -r name tool q want <<< "$c"
  t0=0; t1=0; n0=0; n1=0; ok0=0; ok1=0
  for _ in $(seq "$RUNS"); do
    IFS='|' read -r tok0 turn0 a0 <<< "$(run_one "$tool" "$q" 1)"
    IFS='|' read -r tok1 turn1 a1 <<< "$(run_one "$tool" "$q" 0)"
    t0=$((t0 + tok0)); t1=$((t1 + tok1)); n0=$((n0 + ${turn0:-0})); n1=$((n1 + ${turn1:-0}))
    [ -z "${AB_VERBOSE:-}" ] || printf '   without: %s\n   with:    %s\n' "$a0" "$a1"
    [ "$(correct "$a0" "$want")" = yes ] && ok0=$((ok0 + 1))
    [ "$(correct "$a1" "$want")" = yes ] && ok1=$((ok1 + 1))
  done
  t0=$((t0 / RUNS)); t1=$((t1 / RUNS))
  if [ "$t0" -gt 0 ]; then saved=$(( (t0 - t1) * 100 / t0 )); else saved=0; fi
  printf '%-16s %9s %9s %5s%%  %-9s %-10s  %s -> %s   (%s)\n' "$name" "$t0" "$t1" "$saved" "$ok0/$RUNS" "$ok1/$RUNS" "$n0" "$n1" "$want"
  [ "$ok1" -ge "$ok0" ] || fail=1
  [ "$ok1" -ge 1 ] || fail=1
  [ "$t1" -le $(( t0 * 115 / 100 + 300 )) ] || fail=1
done
[ "$fail" = 0 ] && echo "MCP PASS: answers are as right as without dtk, and no case costs more tokens" || { echo "MCP FAIL"; exit 1; }
