"""The Hermes plugin (integrations/hermes/__init__.py), run as Hermes runs it, with no model.

Hermes imports the plugin, calls `register(ctx)`, and `ctx.register_hook("transform_tool_result", fn)`
gives it the function that is called with `tool_name`, `args` and `result` (a JSON string) for each
tool. A string that is returned replaces the result; None leaves it. Run by tests/hermes_plugin.rs
with DTK_BIN and DTK_ROOT set; prints one line for each check, and exits 1 when one fails.
"""
import importlib.util
import json
import os
import stat
import sys
import tempfile

sys.dont_write_bytecode = True  # no __pycache__ next to the plugin

root, dtk = os.environ["DTK_ROOT"], os.environ["DTK_BIN"]
work = tempfile.mkdtemp(prefix="dtk-hermes-")
os.environ["HOME"] = work
os.makedirs(os.path.join(work, "bin"))
os.symlink(dtk, os.path.join(work, "bin", "dtk"))
os.environ["PATH"] = os.path.join(work, "bin") + os.pathsep + os.environ["PATH"]
os.environ.pop("DTK_DISABLED", None)


def load():
    spec = importlib.util.spec_from_file_location("dtk_plugin", os.path.join(root, "integrations/hermes/__init__.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


failed = 0


def check(name, ok, detail=""):
    global failed
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else ": " + str(detail)))
    if not ok:
        failed += 1


orders = "order_id,customer,region,amount,status\n" + "".join(
    "%d,cust_%03d,%s,%d.5,%s\n" % (1000 + i, i % 40, ["us", "eu", "ap"][i % 3], i * 3, ["ok", "ok", "failed"][i % 3])
    for i in range(300)
)
mod = load()
registered = {}


class Ctx:
    def register_hook(self, name, fn):
        registered[name] = fn


mod.register(Ctx())
hook = registered.get("transform_tool_result")
check("register() adds a transform_tool_result hook", callable(hook))


def result(output, **extra):
    return json.dumps({"output": output, "exit_code": 0, "error": None, **extra})


new = hook(tool_name="terminal", args={"command": "cat orders.csv"}, result=result(orders), task_id="t", duration_ms=5)
body = json.loads(new) if isinstance(new, str) else None
check("a table is replaced by dtk's summary, in the JSON that Hermes gave", body and body["output"].startswith("[dtk] csv: 300 rows"), str(new)[:80])
check("the other fields are as they were", body and body["exit_code"] == 0 and body["error"] is None)
check("the summary is under a quarter of the table", body and len(body["output"]) * 4 < len(orders))

for what, kwargs in [
    ("a small output", dict(tool_name="terminal", args={"command": "cat s.csv"}, result=result("a,b\n1,2\n"))),
    ("rows that were asked for exactly", dict(tool_name="terminal", args={"command": "head -n 100 orders.csv"}, result=result(orders))),
    ("another tool", dict(tool_name="read_file", args={"path": "orders.csv"}, result=result(orders))),
    ("a command that failed", dict(tool_name="terminal", args={"command": "cat orders.csv"}, result=json.dumps({"output": orders, "exit_code": 2, "error": None}))),
    ("a result with an error", dict(tool_name="terminal", args={"command": "cat orders.csv"}, result=json.dumps({"output": orders, "exit_code": 0, "error": "x"}))),
    ("a result that is not JSON", dict(tool_name="terminal", args={"command": "cat orders.csv"}, result=orders)),
    ("a result that is not an object", dict(tool_name="terminal", args={"command": "cat orders.csv"}, result="[1, 2]")),
    ("a result that is not a string", dict(tool_name="terminal", args={"command": "cat orders.csv"}, result=None)),
    ("a call with no command", dict(tool_name="terminal", args={}, result=result(orders))),
]:
    check(what + " is not changed", hook(**kwargs) is None)

os.environ["DTK_DISABLED"] = "1"
check("DTK_DISABLED=1 changes nothing", hook(tool_name="terminal", args={"command": "cat orders.csv"}, result=result(orders)) is None)
del os.environ["DTK_DISABLED"]


def fake(name, body):
    d = os.path.join(work, name)
    os.makedirs(d)
    p = os.path.join(d, "dtk")
    with open(p, "w") as f:
        f.write("#!/bin/sh\n" + body + "\n")
    os.chmod(p, os.stat(p).st_mode | stat.S_IEXEC)
    return d


saved = os.environ["PATH"]
for name, body, what in [
    ("garbage", "cat >/dev/null; echo 'this is not json'", "bad output"),
    ("fails", "cat >/dev/null; exit 3", "a failure"),
    ("wrongshape", "cat >/dev/null; echo '{\"output\": 42}'", "an answer of another shape"),
]:
    os.environ["PATH"] = fake(name, body) + os.pathsep + saved
    check(what + " from dtk changes nothing", hook(tool_name="terminal", args={"command": "cat orders.csv"}, result=result(orders)) is None)
os.environ["PATH"] = "/nonexistent"
check("no dtk on the PATH changes nothing", hook(tool_name="terminal", args={"command": "cat orders.csv"}, result=result(orders)) is None)
os.environ["PATH"] = saved

sys.exit(1 if failed else 0)
