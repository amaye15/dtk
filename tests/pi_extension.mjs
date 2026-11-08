// The Pi extension (integrations/pi/dtk.ts), run as Pi runs it, with no model.
//
// Pi calls the default export with its `pi` object; the extension registers a `tool_result`
// handler with `pi.on`, and Pi gives each result to it. A handler that returns `{content}` replaces
// the content of the result. Run by tests/opencode_plugin.rs with DTK_BIN set; prints one line for
// each check, and exits 1 when one fails.

import { chmodSync, mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"

const root = process.env.DTK_ROOT
const dtk = process.env.DTK_BIN
const work = mkdtempSync(join(tmpdir(), "dtk-pi-"))
const bin = join(work, "bin")
mkdirSync(bin)
symlinkSync(dtk, join(bin, "dtk"))
process.env.PATH = bin + ":" + process.env.PATH
process.env.HOME = work

const url = (q = "") => pathToFileURL(join(root, "integrations/pi/dtk.ts")).href + q
const load = async (q = "") => {
  const handlers = {}
  ;(await import(url(q))).default({ on: (name, fn) => (handlers[name] = fn) })
  return handlers.tool_result
}

let failed = 0
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${ok ? "" : ": " + detail}`)
  if (!ok) failed++
}

const orders = "order_id,customer,region,amount,status\n" +
  Array.from({ length: 300 }, (_, i) => `${1000 + i},cust_${String(i % 40).padStart(3, "0")},${["us", "eu", "ap"][i % 3]},${i * 3}.5,${["ok", "ok", "failed"][i % 3]}`).join("\n") + "\n"
const event = (text, extra = {}) => ({
  toolName: "bash", toolCallId: "c1", input: { command: "cat orders.csv" },
  content: [{ type: "text", text }], details: {}, isError: false, ...extra,
})

const handler = await load()
check("the extension registers a tool_result handler", typeof handler === "function")

let patch = await handler(event(orders))
check("a table is replaced by dtk's summary", patch?.content?.[0]?.type === "text" && patch.content[0].text.startsWith("[dtk] csv: 300 rows"), JSON.stringify(patch).slice(0, 80))
check("the summary is under a quarter of the table", patch.content[0].text.length * 4 < orders.length)
check("only the content is in the patch", Object.keys(patch).join() === "content")

patch = await handler(event("a,b\n1,2\n"))
check("a small output is not changed", patch === undefined)
patch = await handler(event(orders, { input: { command: "head -n 100 orders.csv" } }))
check("rows that were asked for exactly are not changed", patch === undefined)
patch = await handler(event(orders, { toolName: "read" }))
check("another tool is not changed", patch === undefined)
patch = await handler(event(orders, { isError: true }))
check("an error result is not changed", patch === undefined)
patch = await handler(event(orders, { content: [{ type: "text", text: orders }, { type: "image", data: "AAAA" }] }))
check("a result with an image is not changed", patch === undefined)
patch = await handler(event(orders, { input: {} }))
check("a call with no command is not changed", patch === undefined)

process.env.DTK_DISABLED = "1"
check("DTK_DISABLED=1 changes nothing", (await handler(event(orders))) === undefined)
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
  ["wrongshape", "cat >/dev/null; echo '{\"output\": 42}'", "an answer of another shape"],
]) {
  const d = fake(name, body)
  const saved = process.env.PATH
  process.env.PATH = d + ":" + saved
  const h = await load("?" + name)
  check(`${what} from dtk changes nothing`, (await h(event(orders))) === undefined)
  process.env.PATH = saved
}

{
  const saved = process.env.PATH
  process.env.PATH = "/nonexistent"
  const h = await load("?nopath")
  check("no dtk on the PATH changes nothing", (await h(event(orders))) === undefined)
  process.env.PATH = saved
}

process.exit(failed ? 1 : 0)
