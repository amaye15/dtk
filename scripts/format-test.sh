#!/bin/bash
# Experiment: does a model answer questions about a small table better from Markdown than from CSV?
#
# dtk shows a table of up to 60 rows whole, as CSV (the cheapest form). Published tests of tables
# in prompts (TQA-Bench) found Markdown more accurate and CSV cheapest. This script settles it for
# the questions that matter here, with the real model, on a real fixture.
#
# The same table goes to the model in both forms, through `cat`, with NO hook (so nothing but the
# form differs), and the model is asked questions whose answers are worked out here from the CSV,
# not by dtk. The decision rule is fixed before the run:
#
#   switch the whole view to Markdown only if Markdown is right at least 10 points more often
#   AND the Markdown table costs at most 25% more tokens than the CSV.
#
# Otherwise the result is recorded and CSV stays.
#
# Needs: the Claude Code CLI (CLAUDE_BIN), jaq, python3.
# Environment: MODEL (default haiku, the weakest and so the most sensitive), RUNS (default 3),
# ROWS (default 40, at most 60), VERBOSE=1 (print the answers).
# Usage: scripts/format-test.sh        (about 15 seconds per run; 7 questions x 2 forms x RUNS)
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${CLAUDE_BIN:-}" ]; then
  CLAUDE_BIN="$(ls -d "$HOME"/.vscode/extensions/anthropic.claude-code-*/resources/native-binary/claude 2>/dev/null | sort -V | tail -1)"
  CLAUDE_BIN="${CLAUDE_BIN:-claude}"
fi
MODEL="${MODEL:-haiku}"
RUNS="${RUNS:-3}"
ROWS="${ROWS:-40}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
SRC="$HERE/tests/corpus/positive/csv_orders.csv"

# A project with no hook: the table reaches the model exactly as written.
mkdir -p "$WORK/proj/.claude"; echo '{}' > "$WORK/proj/.claude/settings.json"

# The two forms, and the questions with their answers, all from the CSV.
python3 - "$SRC" "$ROWS" "$WORK" <<'PY'
import csv, sys
src, n, work = sys.argv[1], int(sys.argv[2]), sys.argv[3]
rows = list(csv.DictReader(open(src)))[:n]
head = list(rows[0].keys())
lead = f"[dtk] csv as csv: {len(rows)} rows x {len(head)} cols, nothing dropped\n"
csv_text = lead + ",".join(head) + "\n" + "\n".join(",".join(r[h] for h in head) for r in rows) + "\n"
md_text = lead + "| " + " | ".join(head) + " |\n| " + " | ".join("---" for _ in head) + " |\n" + \
    "\n".join("| " + " | ".join(r[h] for h in head) + " |" for r in rows) + "\n"
# Neutral names: the file name must not say which form it is.
open(f"{work}/table_1.txt", "w").write(csv_text)
open(f"{work}/table_2.txt", "w").write(md_text)
amount = lambda r: float(r["amount"])
odd = [r for r in rows if r["status"] != "ok"][len(rows) // 6]
by_region = {}
for r in rows:
    by_region.setdefault(r["region"], 0.0)
    by_region[r["region"]] += amount(r)
region = sorted(by_region)[0]
qs = [
    ("lookup", f"What is the status of order_id {odd['order_id']}? Give the status.", odd["status"]),
    ("count", "How many orders have status failed? Give the number.", str(sum(r["status"] == "failed" for r in rows))),
    ("max", "Which customer has the largest amount? Give the customer.", max(rows, key=amount)["customer"]),
    ("sum", f"What is the sum of amount for region {region}? Round to one decimal place.", f"{by_region[region]:.1f}"),
    ("distinct", "How many distinct customers appear in the table? Give the number.", str(len({r["customer"] for r in rows}))),
    ("min", "Which order_id has the smallest latency_ms? Give the order_id.", min(rows, key=lambda r: int(r["latency_ms"]))["order_id"]),
    ("above", "How many orders have an amount above 250? Give the number.", str(sum(amount(r) > 250 for r in rows))),
]
open(f"{work}/questions.tsv", "w").write("\n".join("\t".join(q) for q in qs) + "\n")
PY

run_one() { # question file -> prints the answer on one line
  local q="$1" file="$2" out="$WORK/run.$$.jsonl"
  for _ in 1 2; do
    ( cd "$WORK/proj" && "$CLAUDE_BIN" -p "Run this bash command: cat $file   Then answer in one short line: $q" \
        --model "$MODEL" --allowedTools Bash --setting-sources project \
        --output-format stream-json --verbose < /dev/null > "$out" 2>/dev/null )
    grep -q '"type":"result"' "$out" 2>/dev/null && break
  done
  jaq -r 'select(.type=="result") | .result' "$out" 2>/dev/null | tr '\n' ' '
}

correct() { # answer expected -> yes/no (the expected words all appear)
  local a w
  a="$(echo "$1" | tr '[:upper:]' '[:lower:]')"
  for w in $(echo "$2" | tr '[:upper:]' '[:lower:]'); do case "$a" in *"$w"*) ;; *) echo no; return;; esac; done
  echo yes
}

printf 'model %s, %s run(s) per form, %s rows\n' "$MODEL" "$RUNS" "$ROWS"
printf '%-9s %-9s %-9s  %s\n' "question" "csv" "markdown" "(expected)"
ok1=0; ok2=0; total=0
while IFS=$'\t' read -r name q want; do
  c1=0; c2=0
  for _ in $(seq "$RUNS"); do
    a1="$(run_one "$q" "$WORK/table_1.txt")"; a2="$(run_one "$q" "$WORK/table_2.txt")"
    [ -z "${VERBOSE:-}" ] || printf '   csv: %s\n   md:  %s\n' "$a1" "$a2"
    [ "$(correct "$a1" "$want")" = yes ] && c1=$((c1 + 1))
    [ "$(correct "$a2" "$want")" = yes ] && c2=$((c2 + 1))
    total=$((total + 1))
  done
  ok1=$((ok1 + c1)); ok2=$((ok2 + c2))
  printf '%-9s %-9s %-9s  %s\n' "$name" "$c1/$RUNS" "$c2/$RUNS" "$want"
done < "$WORK/questions.tsv"

t1=$(( $(wc -c < "$WORK/table_1.txt") / 4 )); t2=$(( $(wc -c < "$WORK/table_2.txt") / 4 ))
p1=$(( ok1 * 100 / total )); p2=$(( ok2 * 100 / total ))
echo
echo "right:  csv $ok1/$total ($p1%)   markdown $ok2/$total ($p2%)"
echo "tokens: csv $t1   markdown $t2 (+$(( (t2 - t1) * 100 / t1 ))%)"
if [ $((p2 - p1)) -ge 10 ] && [ "$t2" -le $(( t1 * 125 / 100 )) ]; then
  echo "RULE MET: Markdown is $((p2 - p1)) points more accurate for $(( (t2 - t1) * 100 / t1 ))% more tokens. Switch."
else
  echo "RULE NOT MET (needs +10 points and at most +25% tokens): CSV stays."
fi
