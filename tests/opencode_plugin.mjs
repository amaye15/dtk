// The OpenCode plugin (integrations/opencode/dtk.ts), run as OpenCode runs it, with no model.
//
// OpenCode calls `server()` of the plugin and then `"tool.execute.after"(input, output)` after each
// tool; `output` is the object that goes back to the model. The plugin calls `dtk hook opencode`.
// Run by tests/opencode_plugin.rs with DTK_BIN set; prints one line for each check, and exits 1
// when one fails.

import { chmodSync, mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

const root = process.env.DTK_ROOT
const dtk = process.env.DTK_BIN
const work = mkdtempSync(join(tmpdir(), "dtk-oc-"))
const bin = join(work, "bin")
mkdirSync(bin)
symlinkSync(dtk, join(bin, "dtk"))
process.env.PATH = bin + ":" + process.env.PATH
process.env.HOME = work

const plugin = (await import(pathToFileURL(join(root, "integrations/opencode/dtk.ts")).href)).default

let failed = 0
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${ok ? "" : ": " + detail}`)
  if (!ok) failed++
}

const orders = "order_id,customer,region,amount,status\n" +
  Array.from({ length: 300 }, (_, i) => `${1000 + i},cust_${String(i % 40).padStart(3, "0")},${["us", "eu", "ap"][i % 3]},${i * 3}.5,${["ok", "ok", "failed"][i % 3]}`).join("\n") + "\n"
const rows = JSON.stringify(Array.from({ length: 300 }, (_, i) => ({ id: 1000 + i, name: `user_${i % 40}`, amount: i * 3 })))

check("the plugin is an object with an id, a setup and a server", plugin && typeof plugin === "object" && plugin.id === "dtk" && typeof plugin.setup === "function" && typeof plugin.server === "function")

const hooks = await plugin.server()
const after = hooks["tool.execute.after"]
check("server() gives tool.execute.after", typeof after === "function")

// A bash result: OpenCode's own object, changed in place.
let output = { title: "cat orders.csv", metadata: { exit: 0 }, output: orders }
await after({ tool: "bash", sessionID: "s", callID: "c", args: { command: "cat orders.csv" } }, output)
check("a table is replaced by dtk's summary", output.output.startsWith("[dtk] csv: 300 rows"), output.output.slice(0, 80))
check("the summary is under a quarter of the table", output.output.length * 4 < orders.length, `${output.output.length} of ${orders.length}`)
check("the other fields are as they were", output.title === "cat orders.csv" && output.metadata.exit === 0)

// What dtk leaves alone is left alone.
output = { title: "x", metadata: {}, output: "a,b\n1,2\n" }
await after({ tool: "bash", args: { command: "cat small.csv" } }, output)
check("a small output is not changed", output.output === "a,b\n1,2\n")
output = { title: "x", metadata: {}, output: orders }
await after({ tool: "bash", args: { command: "head -n 100 orders.csv" } }, output)
check("rows that were asked for exactly are not changed", output.output === orders)
output = { title: "x", metadata: {}, output: orders }
await after({ tool: "edit", args: {} }, output)
check("another tool is not changed", output.output === orders)

// The words of the description reach dtk, and it adds the row that was named.
output = { title: "x", metadata: {}, output: orders }
await after({ tool: "bash", args: { command: "cat orders.csv", description: "Look up order 1150" } }, output)
check("the description names a row, and the summary has it", output.output.includes("matches for words in your request (order_id = 1150)"), output.output.slice(0, 60))

// An MCP result is a list of content blocks, and keeps its shape.
let mcp = { content: [{ type: "text", text: rows }], isError: false }
await after({ tool: "db_query", args: {} }, mcp)
check("an MCP result keeps its blocks", Array.isArray(mcp.content) && mcp.content[0].type === "text" && mcp.content[0].text.startsWith("[dtk] "), JSON.stringify(mcp).slice(0, 80))
mcp = { content: [{ type: "text", text: rows }, { type: "image", data: "AAAA" }] }
await after({ tool: "db_query", args: {} }, mcp)
check("an MCP result with an image is not changed", mcp.content.length === 2 && mcp.content[0].text === rows)

// The 2.x entry: setup(ctx) registers execute.after on ctx.tool.
let registered
await plugin.setup({ tool: { hook: async (name, fn) => { registered = { name, fn } } } })
check("setup() registers execute.after", registered && registered.name === "execute.after")
const event = { status: "completed", tool: "bash", input: { command: "cat orders.csv" }, result: { output: orders } }
await registered.fn(event)
check("the 2.x event is changed in place", event.result.output.startsWith("[dtk] csv: 300 rows"), String(event.result.output).slice(0, 60))
const failedEvent = { status: "failed", tool: "bash", input: { command: "cat orders.csv" }, result: { output: orders } }
await registered.fn(failedEvent)
check("a failed tool is not changed", failedEvent.result.output === orders)

// Everything that can go wrong leaves the result as it was.
process.env.DTK_DISABLED = "1"
output = { title: "x", metadata: {}, output: orders }
await after({ tool: "bash", args: { command: "cat orders.csv" } }, output)
check("DTK_DISABLED=1 changes nothing", output.output === orders)
delete process.env.DTK_DISABLED

const fake = (name, body) => {
  const d = join(work, name)
  mkdirSync(d)
  writeFileSync(join(d, "dtk"), `#!/bin/sh\n${body}\n`)
  chmodSync(join(d, "dtk"), 0o755)
  return d
}
for (const [name, body, what] of [
  ["garbage", "cat >/dev/null; echo 'this is not json'", "bad output"],
  ["fails", "cat >/dev/null; exit 3", "a failure"],
  ["empty", "cat >/dev/null", "no answer"],
  ["wrongshape", "cat >/dev/null; echo '{\"output\": 42}'", "an answer of another shape"],
]) {
  const d = fake(name, body)
  const saved = process.env.PATH
  process.env.PATH = d + ":" + saved
  // The plugin finds dtk on every call, but keeps the first one it found: a new module has none.
  const p = (await import(pathToFileURL(join(root, "integrations/opencode/dtk.ts")).href + "?" + name)).default
  const h = (await p.server())["tool.execute.after"]
  output = { title: "x", metadata: {}, output: orders }
  await h({ tool: "bash", args: { command: "cat orders.csv" } }, output)
  check(`${what} from dtk changes nothing`, output.output === orders, String(output.output).slice(0, 60))
  process.env.PATH = saved
}

{
  const d = fake("slow", "cat >/dev/null; sleep 30")
  const saved = process.env.PATH
  process.env.PATH = d + ":" + saved
  const mod = await import(pathToFileURL(join(root, "integrations/opencode/dtk.ts")).href + "?slow")
  const started = Date.now()
  const answer = await mod.callDtk(join(d, "dtk"), { tool: "bash", args: {}, output: { output: orders } }, 400)
  check("a dtk that is too slow is given up on", answer === null && Date.now() - started < 5000, `${Date.now() - started} ms`)
  process.env.PATH = saved
}

{
  const saved = process.env.PATH
  process.env.PATH = "/nonexistent"
  const p = (await import(pathToFileURL(join(root, "integrations/opencode/dtk.ts")).href + "?nopath")).default
  const h = (await p.server())["tool.execute.after"]
  output = { title: "x", metadata: {}, output: orders }
  await h({ tool: "bash", args: { command: "cat orders.csv" } }, output)
  check("no dtk on the PATH changes nothing", output.output === orders)
  process.env.PATH = saved
}

process.exit(failed ? 1 : 0)
