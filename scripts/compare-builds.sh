#!/bin/bash
# Compare two dtk builds on real files: what each one detects, and whether it would replace the
# output. A release must change only what it is meant to change, so run this before a release and
# put every difference that it lists in the changelog.
#
#   scripts/compare-builds.sh OLD_DTK NEW_DTK [options] PATH...
#
# PATH is a file or a folder. Good sources: the big results of your own Claude Code history
# (`dtk discover --dump DIR` writes them as files) and the text files of your projects.
#
# Options:
#   --allow REGEX   a difference in a path that matches REGEX is expected, and does not fail
#   --min BYTES     skip smaller files (default 1500, the size under which nothing is replaced)
#   --max BYTES     skip bigger files (default 400000)
#   --command TEXT  the command that the output is said to come from (default "mytool --run").
#                   dtk leaves the output of `cat`, `grep` and the like to its own rules.
#
# Exit status: 0 when no unexpected difference was found, 1 when there was one, 2 on a bad call.
set -u
usage() { sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
[ $# -ge 3 ] || usage
OLD="$1"; NEW="$2"; shift 2
ALLOW=""; MIN=1500; MAX=400000; CMD="mytool --run"; PATHS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --allow) ALLOW="$2"; shift 2;;
    --min) MIN="$2"; shift 2;;
    --max) MAX="$2"; shift 2;;
    --command) CMD="$2"; shift 2;;
    -h|--help) usage;;
    *) PATHS+=("$1"); shift;;
  esac
done
[ "${#PATHS[@]}" -gt 0 ] || usage
for b in "$OLD" "$NEW"; do [ -x "$b" ] || { echo "not an executable: $b"; exit 2; }; done

# What a build makes of a file: the format that it detects and whether it replaces the output.
# The size of the summary is left out on purpose: a release may change how a summary reads.
decide() { # binary file
  "$1" explain "$2" --command "$CMD" 2>/dev/null | awk '
    /^detected:/ { sub(/^detected: */, ""); d = $0 }
    /^decision:/ { sub(/^decision: */, ""); sub(/ *\(.*/, ""); x = $0 }
    END { print (d == "" ? "?" : d) " / " (x == "" ? "?" : x) }'
}

files=0; same=0; different=0; unexpected=0
while IFS= read -r f; do
  grep -Iq . "$f" 2>/dev/null || continue          # text files only
  files=$((files + 1))
  a="$(decide "$OLD" "$f")"; b="$(decide "$NEW" "$f")"
  if [ "$a" = "$b" ]; then same=$((same + 1)); continue; fi
  different=$((different + 1))
  mark="  "
  if [ -n "$ALLOW" ] && [[ "$f" =~ $ALLOW ]]; then mark="ok"; else unexpected=$((unexpected + 1)); mark="!!"; fi
  printf '%s %s\n     old: %s\n     new: %s\n' "$mark" "$f" "$a" "$b"
done < <(find "${PATHS[@]}" -type f -size +"${MIN}"c -size -"${MAX}"c \
           -not -path '*/.git/*' -not -path '*/target/*' -not -path '*/node_modules/*' \
           -not -path '*/.venv/*' -not -path '*/__pycache__/*' 2>/dev/null | sort)

echo "$files text files compared: $same the same, $different different ($unexpected not expected)"
[ "$unexpected" -eq 0 ]
