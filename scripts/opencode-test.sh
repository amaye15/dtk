#!/bin/bash
# The OpenCode plugin against the real OpenCode, in a throwaway project.
#
# Two runs of the same prompt: with `integrations/opencode/dtk.ts` as a plugin, and with
# `--pure` (no plugins). The model is asked to `cat` a 300-row CSV and to say how many rows have
# status "failed". The test reads the raw events of OpenCode and checks what the tool returned to
# the model: with the plugin it must be dtk's summary, without it the CSV as it is. It does not
# trust the model's answer alone; it also checks the answer (100).
#
# Needs: opencode (1.18.29 or later), python3, perl, and a model that OpenCode can use. The default
# is one of OpenCode's free models, which have a usage limit: if it says "Free usage limit
# reached", set MODEL to another (see `opencode models`).
#
# (OpenCode 1.x calls the tool `bash`, 2.x calls it `shell`; the rule is given for both.)
# The project has a permission rule that lets only `cat *` run without a question (the last rule
# that matches wins, so the general one comes first), and the
# configuration of OpenCode is a folder of its own (HOME and XDG_CONFIG_HOME are set), so nothing of
# yours is used or changed. (Without that, OpenCode's desktop CLI loads your own plugins: rtk's
# rewrote `cat orders.csv` to `rtk read orders.csv`, which the permission rule did not match.)
#
# WITH_RTK=1 adds rtk to the run: rtk's own OpenCode plugin (from `rtk init -g --opencode`, in the
# same throwaway configuration) rewrites `cat orders.csv` to `rtk read orders.csv` before the tool
# runs, and dtk replaces the result after. The test then also checks that the command that ran
# was rtk's, that rtk alone returns the CSV, and that with both the model gets dtk's summary. The
# permission rule lets every command run (a rewritten command would not match `cat *`).
#
# Environment: OPENCODE (default `opencode`; the CLI inside OpenCode's desktop app is another one to try),
# DTK_BIN (default target/release/dtk), MODEL (default
# opencode/nemotron-3.5-lightning-free), SECONDS_LIMIT (default 90 per try, 3 tries per run).
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
DTK_BIN="${DTK_BIN:-$HERE/target/release/dtk}"
MODEL="${MODEL:-opencode/nemotron-3.5-lightning-free}"
LIMIT="${SECONDS_LIMIT:-90}"
OPENCODE="${OPENCODE:-opencode}"
command -v "$OPENCODE" >/dev/null || [ -x "$OPENCODE" ] || { echo "opencode is not installed (set OPENCODE to its path)"; exit 2; }
[ -x "$DTK_BIN" ] || { echo "DTK_BIN not found: $DTK_BIN (run cargo build --release)"; exit 2; }
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/proj/.opencode/plugins" "$WORK/bin" "$WORK/xdg/config" "$WORK/xdg/data" "$WORK/home"
cp "$HERE/integrations/opencode/dtk.ts" "$WORK/proj/.opencode/plugins/dtk.ts"
ln -s "$DTK_BIN" "$WORK/bin/dtk"
WITH_RTK="${WITH_RTK:-}"
if [ -n "$WITH_RTK" ]; then
  command -v rtk >/dev/null || { echo "WITH_RTK=1 needs rtk on the PATH"; exit 2; }
  PERMISSION='{"bash": "allow", "shell": "allow"}'
else
  PERMISSION='{"bash": {"*": "ask", "cat *": "allow"}, "shell": {"*": "ask", "cat *": "allow"}}'
fi
printf '{"$schema": "https://opencode.ai/config.json", "permission": %s}\n' "$PERMISSION" > "$WORK/proj/opencode.json"
if [ -n "$WITH_RTK" ]; then
  # rtk's plugin goes to the throwaway configuration folder, and a second project has rtk alone.
  ( HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/xdg/config" XDG_DATA_HOME="$WORK/xdg/data" \
      perl -e 'alarm shift; exec @ARGV' 30 rtk init -g --opencode < /dev/null > "$WORK/rtk-init.log" 2>&1 )
  # rtk writes to ~/.config/opencode; OpenCode reads $XDG_CONFIG_HOME/opencode, so the plugin is copied.
  mkdir -p "$WORK/xdg/config/opencode/plugins"
  cp "$WORK/home/.config/opencode/plugins/rtk.ts" "$WORK/xdg/config/opencode/plugins/rtk.ts" 2>/dev/null \
    || { echo "rtk init did not write its plugin (see $WORK/rtk-init.log)"; trap - EXIT; exit 2; }
  mkdir -p "$WORK/proj-rtk"; cp "$WORK/proj/opencode.json" "$WORK/proj/orders.csv" "$WORK/proj-rtk/"
fi
python3 - "$WORK/proj/orders.csv" <<'PY'
import sys
rows = "order_id,customer,region,amount,status\n" + "".join(
    f"{1000+i},cust_{i%40:03d},{['us','eu','ap'][i%3]},{i*3}.5,{['ok','ok','failed'][i%3]}\n" for i in range(300))
open(sys.argv[1], "w").write(rows)
PY

run_arm() { # name extra-flag [project folder] -> prints one line of result
  local name="$1" flag="$2" out="$WORK/$1.json" proj="${3:-proj}"
  # OpenCode sometimes stalls at its start, before any event; an arm without events is tried again.
  for _ in 1 2 3; do
    ( cd "$WORK/$proj" && HOME="$WORK/home" PATH="$WORK/bin:$PATH" XDG_CONFIG_HOME="$WORK/xdg/config" XDG_DATA_HOME="$WORK/xdg/data" \
        perl -e 'alarm shift; exec @ARGV' "$LIMIT" "$OPENCODE" run $flag -m "$MODEL" --format json \
        "Use the bash tool to run exactly: cat orders.csv   Then answer in one short line: how many rows have status failed?" \
        > "$out" 2> "$WORK/$name.err" )
    pkill -f "opencode run" 2>/dev/null
    [ -s "$out" ] && break
  done
  python3 - "$out" "$name" <<'PY'
import json, sys
path, name = sys.argv[1], sys.argv[2]
tool_out, answer, ran = None, "", ""
for line in open(path, errors="replace"):
    try:
        part = (json.loads(line).get("part") or {})
    except Exception:
        continue
    if part.get("type") == "tool" and part.get("tool") in ("bash", "shell") and (part.get("state") or {}).get("status") == "completed":
        tool_out = part["state"].get("output") or ""
        ran = ((part["state"].get("input") or {}).get("command") or "")
    if part.get("type") == "text":
        answer += part.get("text") or ""
if tool_out is None:
    print(f"{name}|no|-|-|-|{answer.strip()[:80]}")
else:
    print(f"{name}|yes|{len(tool_out)//4}|{tool_out[:24].strip().replace('|','/')}|{ran[:24].replace('|','/')}|{answer.strip()[:80]}")
PY
}

printf '%-9s %-8s %-9s %-26s %-26s %s\n' "arm" "bash ran" "tool tok" "tool output starts" "command asked" "answer"
show() { printf '%-9s %-8s %-9s %-26s %-26s %s\n' "$1" "$2" "$3" "$4" "$5" "$6"; }
fail=0
IFS='|' read -r n1 ran1 tok1 start1 cmd1 ans1 <<< "$(run_arm with-dtk '')"
show "$n1" "$ran1" "$tok1" "$start1" "$cmd1" "$ans1"
# rtk keeps a history of the commands that it ran (in the throwaway HOME). The model asked for
# `cat orders.csv`; if rtk's plugin rewrote it, the history has `rtk read`.
rtk_ran=0
if [ -n "$WITH_RTK" ]; then
  rtk_ran=$(HOME="$WORK/home" XDG_DATA_HOME="$WORK/xdg/data" rtk gain --history 2>/dev/null | grep -c 'rtk read')
fi
IFS='|' read -r n2 ran2 tok2 start2 cmd2 ans2 <<< "$(run_arm without '--pure')"
show "$n2" "$ran2" "$tok2" "$start2" "$cmd2" "$ans2"
if [ -n "$WITH_RTK" ]; then
  IFS='|' read -r n3 ran3 tok3 start3 cmd3 ans3 <<< "$(run_arm rtk-only '' proj-rtk)"
  show "$n3" "$ran3" "$tok3" "$start3" "$cmd3" "$ans3"
fi

if [ "$ran1" != yes ] || [ "$ran2" != yes ] || { [ -n "$WITH_RTK" ] && [ "$ran3" != yes ]; }; then
  echo "INCONCLUSIVE: the model did not run bash in every run (see $WORK/*.err; the free tier may be over its limit)"; trap - EXIT; exit 3
fi
[[ "$start1" == "[dtk] csv: 300 rows"* ]] || { echo "FAIL: with the plugin, the tool did not return dtk's summary"; fail=1; }
[[ "$start2" == "order_id,customer"* ]] || { echo "FAIL: without the plugin, the tool did not return the CSV"; fail=1; }
[ "$tok1" -lt $(( tok2 / 3 )) ] || { echo "FAIL: the summary is not under a third of the CSV ($tok1 against $tok2 tokens)"; fail=1; }
[[ "$ans1" == *100* ]] || { echo "FAIL: the answer with the plugin is not right (100): $ans1"; fail=1; }
if [ -n "$WITH_RTK" ]; then
  [ "$rtk_ran" -ge 1 ] || { echo "FAIL: rtk's history has no \`rtk read\`, so rtk's plugin did not rewrite the command and the test proves nothing about rtk"; fail=1; }
  [[ "$start3" == "order_id,customer"* ]] || { echo "FAIL: with rtk alone, the tool did not return the CSV: $start3"; fail=1; }
fi
if [ "$fail" = 0 ]; then
  if [ -n "$WITH_RTK" ]; then
    echo "PASS: rtk's plugin rewrote the command (rtk's history has \`rtk read\`), and OpenCode's model still got dtk's summary ($tok1 tokens instead of $tok3 with rtk alone) and answered right"
  else
    echo "PASS: OpenCode's model got dtk's summary ($tok1 tokens instead of $tok2) and answered right"
  fi
fi
exit "$fail"
