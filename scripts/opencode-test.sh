#!/bin/bash
# The OpenCode plugin against the real OpenCode, in a throwaway project.
#
# Two runs of the same prompt: with dtk installed by `dtk init --agent opencode`, and with no
# plugin. The model is asked to `cat` a 300-row CSV and to say how many rows
# have status "failed". The test reads the raw events of OpenCode and checks what the tool
# returned to the model: with the plugin it must be dtk's summary, without it the CSV as it is. It
# does not trust the model's answer alone; it also checks the answer (100).
#
# Tried with OpenCode 1.18.29 (the tool is `bash`, `tool.execute.after` gets the result to change,
# dtk's `server()` entry)
# and 2.3.0 (the tool is `shell`, the plugin's `setup()` registers `execute.after`, and the model
# reads the `content` blocks of the result).
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
# WITH_RTK=1 puts rtk's own OpenCode plugin (from `rtk init -g --opencode`) in the configuration
# of the run that has dtk's plugin, so that both are loaded, as on a machine that has both. The test
# then checks that the model still gets dtk's summary, and says whether rtk rewrote the command
# (rtk's history has `rtk read`). In OpenCode 2.3.0 rtk's plugin was seen not to rewrite `cat
# orders.csv` in this setup (rtk's history was empty, although `rtk rewrite` rewrites it), which
# is for rtk to look at: it is said, and it is not a failure of dtk. The permission rule lets every
# command run (a rewritten command would not match `cat *`).
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
mkdir -p "$WORK/proj" "$WORK/proj-plain" "$WORK/bin" "$WORK/xdg-dtk/config" "$WORK/xdg-dtk/data" "$WORK/xdg-plain/config" "$WORK/xdg-plain/data" "$WORK/home"
ln -s "$DTK_BIN" "$WORK/bin/dtk"
# dtk is installed the way a user installs it: `dtk init --agent opencode` writes the plugin to the
# configuration folder that OpenCode reads ($XDG_CONFIG_HOME/opencode/plugins/dtk.ts).
( HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/xdg-dtk/config" PATH="$WORK/bin:$PATH" "$DTK_BIN" init --agent opencode > "$WORK/dtk-init.log" 2>&1 )
[ -f "$WORK/xdg-dtk/config/opencode/plugins/dtk.ts" ] || { echo "dtk init --agent opencode did not write the plugin (see $WORK/dtk-init.log)"; trap - EXIT; exit 2; }
WITH_RTK="${WITH_RTK:-}"
if [ -n "$WITH_RTK" ]; then
  command -v rtk >/dev/null || { echo "WITH_RTK=1 needs rtk on the PATH"; exit 2; }
  PERMISSION='{"bash": "allow", "shell": "allow"}'
else
  PERMISSION='{"bash": {"*": "ask", "cat *": "allow"}, "shell": {"*": "ask", "cat *": "allow"}}'
fi
printf '{"$schema": "https://opencode.ai/config.json", "permission": %s}\n' "$PERMISSION" > "$WORK/proj/opencode.json"
if [ -n "$WITH_RTK" ]; then
  # rtk's plugin goes to the configuration folder that has dtk's plugin; the run with no plugin
  # keeps its own, empty one.
  ( HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/xdg-dtk/config" XDG_DATA_HOME="$WORK/xdg-dtk/data" \
      perl -e 'alarm shift; exec @ARGV' 30 rtk init -g --opencode < /dev/null > "$WORK/rtk-init.log" 2>&1 )
  # rtk writes to ~/.config/opencode; OpenCode reads $XDG_CONFIG_HOME/opencode, so the plugin is copied.
  cp "$WORK/home/.config/opencode/plugins/rtk.ts" "$WORK/xdg-dtk/config/opencode/plugins/rtk.ts" 2>/dev/null \
    || { echo "rtk init did not write its plugin (see $WORK/rtk-init.log)"; trap - EXIT; exit 2; }
fi
python3 - "$WORK/proj/orders.csv" <<'PY'
import sys
rows = "order_id,customer,region,amount,status\n" + "".join(
    f"{1000+i},cust_{i%40:03d},{['us','eu','ap'][i%3]},{i*3}.5,{['ok','ok','failed'][i%3]}\n" for i in range(300))
open(sys.argv[1], "w").write(rows)
PY
cp "$WORK/proj/opencode.json" "$WORK/proj/orders.csv" "$WORK/proj-plain/"

run_arm() { # name project-folder configuration-folder -> prints one line of result
  local name="$1" proj="$2" xdg="$3" out="$WORK/$1.json"
  # OpenCode sometimes stalls at its start, before any event; an arm without events is tried again.
  for _ in 1 2 3; do
    ( cd "$WORK/$proj" && HOME="$WORK/home" PATH="$WORK/bin:$PATH" XDG_CONFIG_HOME="$WORK/$xdg/config" XDG_DATA_HOME="$WORK/$xdg/data" \
        perl -e 'alarm shift; exec @ARGV' "$LIMIT" "$OPENCODE" run -m "$MODEL" --format json \
        "Use the bash tool to run exactly: cat orders.csv   Then answer in one short line: how many rows have status failed?" \
        > "$out" 2> "$WORK/$name.err" )
    pkill -f "opencode run" 2>/dev/null
    # A weak model sometimes runs another command (`grep -c`) than the one that was asked for: that
    # run says nothing about `cat`, and is tried again.
    [ -s "$out" ] && python3 - "$out" <<'PY' && break
import json, sys
for line in open(sys.argv[1], errors="replace"):
    try:
        part = (json.loads(line).get("part") or {})
    except Exception:
        continue
    if part.get("type") == "tool" and part.get("tool") in ("bash", "shell") \
            and (((part.get("state") or {}).get("input") or {}).get("command") or "").strip() == "cat orders.csv":
        sys.exit(0)
sys.exit(1)
PY
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
IFS='|' read -r n1 ran1 tok1 start1 cmd1 ans1 <<< "$(run_arm with-dtk proj xdg-dtk)"
show "$n1" "$ran1" "$tok1" "$start1" "$cmd1" "$ans1"
# rtk keeps a history of the commands that it ran (in the throwaway HOME). The model asked for
# `cat orders.csv`; if rtk's plugin rewrote it, the history has `rtk read`.
rtk_ran=0
if [ -n "$WITH_RTK" ]; then
  rtk_ran=$(HOME="$WORK/home" XDG_DATA_HOME="$WORK/xdg-dtk/data" rtk gain --history 2>/dev/null | grep -c 'rtk read')
fi
IFS='|' read -r n2 ran2 tok2 start2 cmd2 ans2 <<< "$(run_arm without proj-plain xdg-plain)"
show "$n2" "$ran2" "$tok2" "$start2" "$cmd2" "$ans2"

if [ "$ran1" != yes ] || [ "$ran2" != yes ] || [ "$cmd1" != "cat orders.csv" ] || [ "$cmd2" != "cat orders.csv" ]; then
  echo "INCONCLUSIVE: the model did not run \`cat orders.csv\` in every run (see $WORK/*.err; the free tier may be over its limit)"; trap - EXIT; exit 3
fi
[[ "$start1" == "[dtk] csv: 300 rows"* ]] || { echo "FAIL: with the plugin, the tool did not return dtk's summary"; fail=1; }
[[ "$start2" == "order_id,customer"* ]] || { echo "FAIL: with no plugin, the tool did not return the CSV"; fail=1; }
[ "$tok1" -lt $(( tok2 / 3 )) ] || { echo "FAIL: the summary is not under a third of the CSV ($tok1 against $tok2 tokens)"; fail=1; }
[[ "$ans1" == *100* ]] || { echo "FAIL: the answer with the plugin is not right (100): $ans1"; fail=1; }
if [ -n "$WITH_RTK" ]; then
  if [ "$rtk_ran" -ge 1 ]; then
    echo "rtk's plugin rewrote the command (rtk's history has \`rtk read\`) and dtk's summary still reached the model"
  else
    echo "note: rtk's plugin was loaded beside dtk's and did not rewrite the command (rtk's history is empty), so this run shows that the two plugins load together, not that they act on one command"
  fi
fi
if [ "$fail" = 0 ]; then
  echo "PASS: OpenCode's model got dtk's summary ($tok1 tokens instead of $tok2) and answered right"
fi
exit "$fail"
