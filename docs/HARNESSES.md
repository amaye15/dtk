# dtk with other agents

dtk began as a Claude Code hook. The same decision (what is a table, what is a log, what is left
alone) now serves any agent that can do one of three things. This page says which agents can, how
dtk connects each one, and what has and has not been tried with the real program.

## One command: `dtk init --global`

```
dtk init --global             Claude Code, and every other agent below that dtk finds here
dtk init --uninstall          takes dtk out of all of them
dtk init --global --claude-only    Claude Code only
dtk init --agent NAME         one agent (add --uninstall to take dtk out of it)
dtk doctor                    says which agents have dtk
```

An agent is "found" when its folder exists (`~/.codex`, `~/.gemini`, `~/.config/opencode`, ...). For
Codex CLI, Gemini CLI, Copilot CLI and OpenCode it is also found when its program is on the `PATH`
(the names `kilo`, `hermes`, `pi` and `vibe` are used by other programs, so for these four only the
folder counts). Restart each agent after the command, so that it reads its configuration. The installer (`install.sh`, `install.ps1`) runs `dtk init --global` for you.

- A file that already holds other hooks keeps them. dtk adds its own entry next to them, makes a
  backup once (`FILE.dtk-backup`), and does nothing when the command runs again.
- `--uninstall` takes out only dtk's entry. The file is as it was before.
- A file that dtk cannot read (a mistake in the JSON, for example) is not touched. The other agents
  are still connected, and the exit code is 1.
- When Claude Code is not on the machine (no `~/.claude` folder), dtk makes no folder for it.
  When no agent at all is on the machine, dtk changes nothing and says so.

## What an agent must be able to do

| The agent can... | dtk uses | Result |
|---|---|---|
| Run a hook after a tool, and replace the tool's result with the hook's answer | `dtk hook AGENT` | Automatic. The model gets the summary, as in Claude Code. |
| Run a plugin after a tool, and change the result | a plugin that calls `dtk hook plugin` | Automatic. |
| Follow a rule in a file (`AGENTS.md`, a rules folder) | `dtk run -c '...'`, named in the rule | Not automatic: the model must follow the rule. |
| Run a shell command that you write | `dtk run -c '...'`, or `... \| dtk filter` | Whatever your wrapper does. |

## Status

| Agent | How it is connected | Automatic | Tried with the real program |
|---|---|---|---|
| Claude Code | `PostToolUse` hook, chained to rtk | yes | yes (every release) |
| OpenCode | plugin `~/.config/opencode/plugins/dtk.ts`, `tool.execute.after` | yes | yes: OpenCode 1.18.29 with a real model (`scripts/opencode-test.sh`: the tool returned 218 tokens, not 2,050, and the model answered right). OpenCode 2.x (the `setup()` entry of the plugin) is written from the documentation and tried only with a stand-in in Node: the real 2.2.6 could not be driven in a clean environment (it gave no events), so it is not tried. |
| Codex CLI | `PostToolUse` hook in `~/.codex/hooks.json`, the summary is returned as the feedback that replaces the result | yes | no: written from the documentation |
| Gemini CLI | `AfterTool` hook in `~/.gemini/settings.json` | yes | no: written from the documentation |
| Copilot CLI | `postToolUse` hook, `modifiedResult`, in `~/.copilot/hooks/dtk.json` | yes | no: written from the documentation |
| Mistral Vibe | `post_tool` hook in `~/.vibe/hooks.toml` (a block that dtk manages), `decision: "deny"` with the summary as `reason` | yes | no: written from the documentation |
| Kilo Code | the OpenCode plugin, in `~/.config/kilo/plugin/dtk.ts` (Kilo Code is a fork of OpenCode and documents the same `tool.execute.after`) | yes | no: written from the documentation |
| Pi | extension `~/.pi/agent/extensions/dtk.ts`, `tool_result` | yes | no: written from the documentation. The code of the extension is run with Node as Pi calls it. |
| Hermes | plugin `~/.hermes/plugins/dtk/`, `transform_tool_result`; Hermes loads a plugin only after `hermes plugins enable dtk` (dtk runs it when `hermes` is on the `PATH`) | yes | no: written from the documentation. The code of the plugin is run with Python as Hermes calls it. |
| Cursor | a rule file: `dtk init --agent cursor` | no | not applicable |
| Any other agent | `dtk run`, `dtk filter`, or a rule: `dtk init --agent rules` | by rule | not applicable |

"Written from the documentation" means: the event and the reply have the fields that the
agent's own documentation gives, and tests check them (`tests/harness.rs`, `tests/agents.rs`,
`dtk verify`). It does not mean that the real program was run. If one of these does not work for
you, run `dtk skeleton` on what happened and open an issue; the template asks for exactly that.

### Agents with no way to replace a result

These agents have hooks, but by their documentation a hook cannot change what the model sees after
a tool ran. dtk cannot make them automatic. Use `dtk run`, or a rule (`dtk init --agent rules`).

| Agent | What its documentation says |
|---|---|
| Cursor | Hooks can replace the result of an MCP tool, not of a shell command. |
| Cline | The `PostToolUse` hook gets the result and can cancel, not replace it. |
| Windsurf (Cascade) | `post_run_command` runs after the command and cannot change what the model gets. |
| Factory Droid | `PostToolUse` can block with a reason or add context; no replacement is documented. |

Not found in the documentation: Kimi CLI, Google Antigravity, Trae, Oh My Pi. If you use one of
them and it can replace a tool result, an issue is welcome: a new agent is one small adapter.

## Why dtk does not rewrite the command

Some agents (Cursor, Gemini CLI, Copilot CLI, Codex, OpenCode) can rewrite a command before it
runs, and the command could be changed to `dtk run -c '...'`. dtk does not do this by itself. The
text of the command is what the agent's allow and deny rules read. A rule such as "deny `rm *`"
would stop matching `dtk run -c 'rm -rf x'`, and an agent in an automatic mode would run it. A hook
that replaces the *result* after the command ran changes no permission, so that is what dtk uses
wherever it can. Cursor's hooks cannot replace the result of a shell command (only of an MCP tool),
so for Cursor dtk gives you a rule, and `dtk run`, and says that this is not automatic.

## dtk and rtk in every agent

rtk keeps working beside dtk in every agent, and `dtk init --global` is safe to run before or
after `rtk init`.

- **Claude Code:** rtk rewrites the command (`PreToolUse`) and has its own `PostToolUse` hook.
  Claude Code runs only one result-changing hook per tool, so dtk is chained to rtk: one entry,
  `dtk hook claude --then rtk hook claude`. dtk answers when the output is data, and rtk gets every
  other event unchanged.
- **The other agents:** rtk's hooks there (`rtk hook codex`, the Gemini `BeforeTool` script, the
  Copilot `PreToolUse` file, rtk's OpenCode plugin) run *before* the tool and rewrite the command.
  dtk's run *after* the tool and replace the result. They are different events in different
  entries or files, so they never meet, and neither installer or uninstaller touches the other's
  entry. This was tried with the real rtk 1.1.0 for Codex, Gemini CLI, Copilot CLI and OpenCode:
  `rtk init` after dtk, `rtk init --uninstall` after dtk, and `dtk init --uninstall` after rtk each
  left the other tool's entry as it was.
- **Output that rtk cut:** rtk cuts a long output the same way in every agent and saves the full
  copy. dtk reads that copy (`rtk/tee`, or `rtk recall`) in every agent, so a summary or a digest
  covers the lines that rtk cut. `tests/rtk_compat.rs` runs the real rtk and hands its cut output
  to each adapter.
- **A command that rtk rewrote** (`cat x.csv` became `rtk read x.csv`) is understood by dtk in
  every agent: the rows that were asked for exactly (`head`, `sed -n`) are not summarized, and
  `exclude_commands` still matches.

## Notes for each agent

- **Codex** asks you to review a new hook in `/hooks` before it runs, and needs `hooks = true`
  under `[features]` in `config.toml`.
- **OpenCode, Kilo Code, Pi and Hermes** find `dtk` on their own `PATH`. They do not read your
  shell profile, so a `dtk` that only your shell finds is not found by the plugin. Homebrew and
  the install script put it on the `PATH`. `dtk init` says so when `dtk` is not on yours.
- **Hermes** loads the plugin only after it is enabled. If `hermes` is not on the `PATH`, run
  `hermes plugins enable dtk` yourself.
- **Mistral Vibe** reports the status `success` for a shell command that failed. dtk reads the
  exit code in the output and leaves a failed command as it was.

## For any agent: `dtk run` and `dtk filter`

```
dtk run -c 'psql -c "select * from orders"'     run a shell command, print its summary
dtk run -- python report.py                     run a program
cat orders.csv | dtk filter                     summarize text that is already there
```

- The exit code is the command's. A command that fails shows its output as it was.
- What dtk leaves alone comes out byte for byte: output that is not data, rows that were asked for
  exactly (`head`, `tail`, `sed -n`), output that is not text, `DTK_DISABLED=1`, `DTK_MODE=shadow`.
- stderr goes straight through. Standard input reaches the command.
- The output is held until the command ends, so a command that must stream (`tail -f`) is not for
  `dtk run`.
- `--request TEXT` (or `DTK_REQUEST`) gives the words of what the user asked, so that a summary can
  add the row that was asked for.

A shell function is enough for an agent that runs commands in a shell you configure:

```
dq() { dtk run -c "$*"; }
```

## What is the same in every agent

The rules are `hook::handle`, one function for all agents: the settings in `config.toml`
(`exclude_commands`, `digest_logs`, `html_pages`, `request_rows`, the thresholds), the files that
dtk saves and the `dtk where|top|rows|get|html` commands that read them, the log of decisions
(`dtk gain`), and the guards that leave exact requests and dtk's own answers alone.

What is only in Claude Code: the words of the user's last prompt (read from Claude Code's
transcript) for the "matches for words in your request" rows. In the other agents the command, its
description and `DTK_REQUEST` are used.
