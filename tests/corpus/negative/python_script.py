import json, os, random, subprocess, sys
import pandas as pd
out = sys.argv[1]
os.makedirs(out + "/positive", exist_ok=True); os.makedirs(out + "/negative", exist_ok=True); os.makedirs(out + "/either", exist_ok=True)
random.seed(11)
def w(path, text): open(f"{out}/{path}", "w").write(text)
def sh(cmd, cwd=None):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True, cwd=cwd)
    return r.stdout
manifest = {"positive": [], "negative": [], "either": []}
def pos(path, fmt, rows, cols, headers=None, text=None):
    if text is not None: w(path, text)
    e = {"file": path, "format": fmt, "rows": rows, "cols": cols}
    if headers: e["headers"] = headers
    manifest["positive"].append(e)
def neg(path, text=None, note=""):
    if text is not None: w(path, text)
    manifest["negative"].append({"file": path, "note": note})
def either(path, text=None):
    if text is not None: w(path, text)
    manifest["either"].append({"file": path})

n = 300
df = pd.DataFrame({"order_id": range(100000, 100000 + n),
  "ts": pd.date_range("2026-10-01", periods=n, freq="7min"),
  "customer": [f"cust_{random.randint(0,899):04d}" for _ in range(n)],
  "region": [random.choice(["us-east","us-west","eu-central","ap-south"]) for _ in range(n)],
  "amount": [round(random.gammavariate(2.0, 40.0), 2) for _ in range(n)],
  "status": [random.choice(["ok","ok","ok","retry","failed"]) for _ in range(n)],
  "latency_ms": [random.randint(5, 900) for _ in range(n)]})
H = list(df.columns)
pos("positive/csv_orders.csv", "csv", n, 7, H, df.to_csv(index=False))
pos("positive/tsv_orders.tsv", "tsv", n, 7, H, df.to_csv(index=False, sep="\t"))
pos("positive/jsonl_orders.jsonl", "jsonl", n, 7, H, df.to_json(orient="records", lines=True, date_format="iso"))
pos("positive/json_array_orders.json", "json-array", n, 7, H, df.to_json(orient="records", date_format="iso"))
pos("positive/json_array_pretty.json", "json-array", n, 7, H, df.head(120).to_json(orient="records", date_format="iso", indent=2))
manifest["positive"][-1]["rows"] = 120
pos("positive/csv_crlf.csv", "csv", n, 7, H, df.to_csv(index=False, lineterminator="\r\n"))
pos("positive/csv_semicolon.csv", "csv", n, 7, H, df.to_csv(index=False, sep=";"))
pos("positive/csv_quoted.csv", "csv", 80, 3, ["id","note","amount"], "id,note,amount\n" + "".join(f'{i},"hello, world {i}",{i*1.5}\n' for i in range(80)))
# pandas without datetime: aligned with index column
d2 = df.drop(columns=["ts"]).head(60)
pos("positive/pandas_to_string.txt", "table", 60, 7, ["idx"] + [c for c in d2.columns], d2.to_string() + "\n")
# polars (real, from the venv)
import polars as pl
pdf = pl.DataFrame({c: df[c].tolist() for c in df.columns if c != "ts"})
with pl.Config(tbl_rows=-1, tbl_cols=-1, tbl_width_chars=220):
    pos("positive/polars_orders.txt", "table", 100, 6, [c for c in d2.columns], str(pdf.head(100)) + "\n")
# sqlite3 in every table mode (real)
q = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<60) SELECT x AS id, 'user'||(x%7) AS name, round(x*2.5,2) AS amount, CASE WHEN x%3=0 THEN 'ok' ELSE 'retry' END AS status FROM c;"
SH = ["id","name","amount","status"]
for mode, fname, fmt in [("-box","sqlite_box.txt","table"),("-table","sqlite_table.txt","table"),("-markdown","sqlite_markdown.txt","table"),("-column","sqlite_column.txt","table"),("-csv -header","sqlite_csv.csv","csv"),("-json","sqlite_json.json","json-array")]:
    txt = sh(f"sqlite3 -header {mode} :memory: \"{q}\"")
    pos(f"positive/{fname}", fmt, 60, 4, SH, txt)
# tabulate (real library)
from tabulate import tabulate
rows = [[i, f"user{i%7}", round(i*2.5, 2), "ok" if i % 3 == 0 else "retry"] for i in range(1, 61)]
for tf in ["psql", "grid", "pipe", "github", "simple", "rounded_grid", "fancy_grid", "presto", "orgtbl", "outline"]:
    pos(f"positive/tabulate_{tf}.txt", "table", 60, 4, SH, tabulate(rows, headers=SH, tablefmt=tf) + "\n")
# rich (real library; heavy box drawing)
from rich.console import Console
from rich.table import Table
import io
t = Table()
for h in SH: t.add_column(h)
for r in rows: t.add_row(*[str(c) for c in r])
buf = io.StringIO(); Console(file=buf, force_terminal=False, width=100).print(t)
pos("positive/rich_table.txt", "table", 60, 4, SH, buf.getvalue())
buf = io.StringIO(); Console(file=buf, force_terminal=True, color_system="truecolor", width=100).print(t)
pos("positive/rich_table_ansi.txt", "table", 60, 4, SH, buf.getvalue())   # ANSI colours (S2)
# jq -C: colourised JSON array of objects (real ANSI)
j = sh(f"jq -C . {out}/positive/json_array_pretty.json")
pos("positive/json_array_ansi.json", "json-array", 120, 7, H, j)
# git log as TSV from a real repo
g = sh("git log --pretty=format:'%h%x09%an%x09%ad%x09%s' --date=short -n 80", cwd=os.path.expanduser("~/Projects/JSON-Tools-rs"))
pos("positive/git_log_tsv.txt", "tsv", 80, 4, None, "hash\tauthor\tdate\tsubject\n" + g + "\n")
# gh api: a REAL object that wraps an array (S3)
runs = sh("gh api 'repos/amaye15/dtk/actions/runs?per_page=100'")
# keep for S3 (not asserted yet)
w("s3_candidates/gh_actions_runs.json", runs) if os.makedirs(out + "/s3_candidates", exist_ok=True) is None else None
# pandas default print: hides columns -> must be rejected
neg("negative/pandas_default_print.txt", repr(df.head(80).assign(extra=1)) + "\n", "columns hidden behind ...")
neg("negative/pandas_datetime_to_string.txt", df.head(50).to_string() + "\n", "datetime column glued by one space; labels unsafe")
# real negatives
neg("negative/readme.md", None); w("negative/readme.md", open(os.path.expanduser("~/Projects/dtk/README.md")).read())
neg("negative/rust_source.rs", None); w("negative/rust_source.rs", open(os.path.expanduser("~/Projects/dtk/src/hook.rs")).read())
neg("negative/changelog.md", None); w("negative/changelog.md", open(os.path.expanduser("~/Projects/dtk/CHANGELOG.md")).read())
neg("negative/cargo_test_output.txt", sh("cargo test --lib 2>&1", cwd=os.path.expanduser("~/Projects/dtk")), "real cargo output")
neg("negative/git_diff.txt", sh("git diff HEAD~3 HEAD", cwd=os.path.expanduser("~/Projects/dtk")), "real diff")
neg("negative/git_log_default.txt", sh("git log -n 40", cwd=os.path.expanduser("~/Projects/JSON-Tools-rs")), "real git log")
neg("negative/env_output.txt", "\n".join(sorted(l for l in sh("env").splitlines() if not any(k in l.upper() for k in ["TOKEN","KEY","SECRET","PASSWORD"]))) + "\n", "KEY=value lines")
neg("negative/brew_list.txt", sh("brew list --versions | head -80"), "two columns")
neg("negative/ls_l.txt", sh("ls -l /usr/bin | head -90"), "real ls -l")
neg("negative/python_script.py", open(__file__).read())
neg("negative/json_object_single.json", sh("gh api repos/amaye15/dtk"), "one object")
neg("negative/prose.txt", ("The quick brown fox jumps over the lazy dog, and then it rests, sleeps, dreams.\n" * 40), "comma prose")
neg("negative/python_traceback.txt", "Traceback (most recent call last):\n" + "".join(f'  File "/app/mod{i}.py", line {i*3}, in handler_{i}\n    result = process(item, {i})\n' for i in range(40)) + "ValueError: bad input\n")
neg("negative/shell_pipelines.txt", "cat access.log | grep error | sort | uniq -c | head -20\n" * 30, "pipes without a rule line")
neg("negative/yaml_list.yaml", "".join(f"- name: item{i}\n  value: {i}\n  tags: [a, b]\n" for i in range(60)), "yaml")
neg("negative/numbers_only.txt", "1,2,3\n" + "4,5,6\n" * 60, "numeric header")
# ambiguous real outputs: must not panic; if detected they must be well formed
either("either/ps_aux.txt", sh("ps aux | head -60"))
either("either/df_h.txt", sh("df -h"))
either("either/lsof_i.txt", sh("lsof -nP -i 2>/dev/null | head -60"))
either("either/netstat.txt", sh("netstat -an | head -80"))
either("either/qsv_stats_table.txt", sh(f"qsv stats --everything {out}/positive/csv_orders.csv | qsv table"))
json.dump(manifest, open(out + "/manifest.json", "w"), indent=1)
print({k: len(v) for k, v in manifest.items()})
