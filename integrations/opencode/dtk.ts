import { spawn } from "node:child_process"
import { existsSync, statSync } from "node:fs"
import { delimiter, join } from "node:path"

// dtk plugin for OpenCode (https://github.com/amaye15/dtk).
//
// After a tool has run, this asks `dtk hook opencode` whether the output is a table, a log or a web
// page that dtk can say in far fewer tokens, and if it answers, puts its summary in place of the
// output. All decisions are dtk's (the same rules as for Claude Code); this file only carries the
// text there and back. It never blocks and never changes a result on any error: no dtk on the
// PATH, a timeout, bad output, all leave the result as it was.
//
// A rewrite of the command is not done here, on purpose: it would change the text that
// OpenCode's permission rules see.

let cachedDtk: string | null = null

/** dtk from OpenCode's own PATH (a shell profile is not read by OpenCode's tools). */
export function resolveDtk(): string | null {
  if (cachedDtk && existsSync(cachedDtk)) return cachedDtk
  const dirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean)
  const exts = process.platform === "win32" ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";") : [""]
  for (const dir of dirs) {
    for (const ext of exts) {
      const full = join(dir, `dtk${ext}`)
      if (existsSync(full) && !statSync(full).isDirectory()) return (cachedDtk = full)
    }
  }
  return (cachedDtk = null)
}

/** Runs `dtk hook opencode` with the payload on standard input. Resolves to dtk's answer or null. */
export function callDtk(bin: string, payload: unknown, timeoutMs = 10000): Promise<any | null> {
  return new Promise((resolve) => {
    let done = false
    const finish = (value: any | null) => {
      if (!done) {
        done = true
        resolve(value)
      }
    }
    let child
    try {
      child = spawn(bin, ["hook", "opencode"], { stdio: ["pipe", "pipe", "ignore"], windowsHide: true })
    } catch {
      return finish(null)
    }
    const chunks: Buffer[] = []
    const timer = setTimeout(() => {
      try {
        child.kill()
      } catch {}
      finish(null)
    }, timeoutMs)
    child.stdout.on("data", (c: Buffer) => chunks.push(c))
    child.on("error", () => finish(null))
    child.on("close", () => {
      clearTimeout(timer)
      try {
        const text = Buffer.concat(chunks).toString("utf8").trim()
        finish(text ? JSON.parse(text) : null)
      } catch {
        finish(null)
      }
    })
    try {
      child.stdin.on("error", () => {})
      child.stdin.end(JSON.stringify(payload))
    } catch {
      finish(null)
    }
  })
}

/** Puts dtk's summary in `output`, the object that OpenCode returns to the model. */
export async function afterTool(tool: unknown, args: unknown, output: any): Promise<void> {
  if (process.env.DTK_DISABLED === "1") return
  if (typeof tool !== "string" || !output || typeof output !== "object") return
  const bin = resolveDtk()
  if (!bin) return
  const answer = await callDtk(bin, { tool, args, output })
  if (!answer || typeof answer !== "object") return
  if (typeof answer.output === "string" && typeof output.output === "string") {
    output.output = answer.output
  } else if (Array.isArray(answer.content) && Array.isArray(output.content)) {
    output.content = answer.content
  }
}

/**
 * OpenCode 1.x (>= 1.18.29) and 2.x: a plain object, as the loader of 2.x requires. `server()` is
 * the 1.x entry (`tool.execute.after` gets the tool and its arguments, and the result to change);
 * `setup()` is the 2.x entry.
 */
const DtkOpenCodePlugin = {
  id: "dtk",

  async setup(ctx: any) {
    await ctx?.tool?.hook?.("execute.after", async (e: any) => {
      if (e?.status !== "completed" || !e?.result) return
      await afterTool(e?.tool, e?.input, e.result)
    })
  },

  async server() {
    return {
      "tool.execute.after": (input: any, output: any) => afterTool(input?.tool, input?.args, output),
    }
  },
}

export default DtkOpenCodePlugin
export { DtkOpenCodePlugin }
