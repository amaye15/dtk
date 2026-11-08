import { spawn } from "node:child_process"
import { existsSync, statSync } from "node:fs"
import { delimiter, join } from "node:path"

// dtk extension for Pi (https://github.com/amaye15/dtk).
//
// After the `bash` tool has run, this asks `dtk hook plugin` whether the output is a table, a log
// or a web page that dtk can say in far fewer tokens, and if it answers, puts its summary in place
// of the output. All decisions are dtk's (the same rules as for Claude Code); this file only
// carries the text there and back. It never blocks and never changes a result on any error: no
// dtk on the PATH, a timeout, bad output, all leave the result as it was.
//
// A rewrite of the command is not done here, on purpose: it would change the text that the
// permission rules of an agent see.

let cachedDtk: string | null = null

/** dtk from the PATH of Pi (a shell profile is not read by Pi's tools). */
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

/** Runs `dtk hook plugin` with the payload on standard input. Resolves to dtk's answer or null. */
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
      child = spawn(bin, ["hook", "plugin"], { stdio: ["pipe", "pipe", "ignore"], windowsHide: true })
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

/** The patch for a `tool_result` event, or undefined to leave the result as it is. */
export async function afterTool(event: any): Promise<{ content: { type: "text"; text: string }[] } | undefined> {
  if (process.env.DTK_DISABLED === "1") return
  if (event?.toolName !== "bash" || event?.isError) return
  const blocks = event?.content
  // Every block must be text: an image is left alone.
  if (!Array.isArray(blocks) || blocks.length === 0) return
  if (!blocks.every((b: any) => b?.type === "text" && typeof b.text === "string")) return
  const command = event?.input?.command
  if (typeof command !== "string") return
  const bin = resolveDtk()
  if (!bin) return
  const answer = await callDtk(bin, {
    tool: "bash",
    args: { command },
    output: { output: blocks.map((b: any) => b.text).join("\n") },
  })
  if (answer && typeof answer === "object" && typeof answer.output === "string") {
    return { content: [{ type: "text", text: answer.output }] }
  }
}

export default function dtkExtension(pi: any) {
  pi.on("tool_result", async (event: any) => {
    try {
      return await afterTool(event)
    } catch {
      return undefined
    }
  })
}
