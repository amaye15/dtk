"""dtk plugin for Hermes (https://github.com/amaye15/dtk).

After the `terminal` tool has run, this asks `dtk hook plugin` whether the output is a table, a log
or a web page that dtk can say in far fewer tokens, and if it answers, puts its summary in place of
the output. All decisions are dtk's (the same rules as for Claude Code); this file only carries the
text there and back. It never changes a result on any error: no dtk on the PATH, a timeout, a
result that is not the JSON that was expected, all leave the result as it was.

A rewrite of the command is not done here, on purpose: it would change the text that the
permission rules of an agent see.
"""

import json
import os
import shutil
import subprocess


def _ask(payload):
    exe = shutil.which("dtk")
    if not exe:
        return None
    try:
        done = subprocess.run(
            [exe, "hook", "plugin"],
            input=json.dumps(payload),
            capture_output=True,
            encoding="utf-8",
            timeout=10,
        )
        text = (done.stdout or "").strip()
        return json.loads(text) if text else None
    except Exception:
        return None


def transform_tool_result(tool_name="", args=None, result=None, **kwargs):
    """Returns the new result (a string), or None to leave the result as it is."""
    try:
        if os.environ.get("DTK_DISABLED") == "1":
            return None
        if tool_name != "terminal" or not isinstance(result, str):
            return None
        if not isinstance(args, dict) or not isinstance(args.get("command"), str):
            return None
        body = json.loads(result)
        if not isinstance(body, dict) or not isinstance(body.get("output"), str):
            return None
        # A command that failed is left as it is.
        if body.get("exit_code") not in (0, None) or body.get("error"):
            return None
        answer = _ask(
            {
                "tool": "bash",
                "args": {"command": args["command"]},
                "output": {"output": body["output"]},
            }
        )
        if not isinstance(answer, dict) or not isinstance(answer.get("output"), str):
            return None
        body["output"] = answer["output"]
        return json.dumps(body, ensure_ascii=False)
    except Exception:
        return None


def register(ctx):
    ctx.register_hook("transform_tool_result", transform_tool_result)
