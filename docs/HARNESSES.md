# dtk with other agents

dtk began as a Claude Code hook. The same decision (what is a table, what is a log, what is left
alone) now serves any agent that can do one of three things. This page says which agents can, how to
connect each one, and what has and has not been tried with the real program.

## What an agent must be able to do

| The agent can... | dtk uses | Result |
|---|---|---|
| Run a hook after a tool, and replace the tool's result with the hook's answer | `dtk hook AGENT` | Automatic. The model gets the summary, as in Claude Code. |
| Run a plugin after a tool, and change the result | a plugin that calls `dtk hook AGENT` | Automatic. |
| Follow a rule in a file (`AGENTS.md`, a rules folder) | `dtk run -c '...'`, named in the rule | Not automatic: the model must follow the rule. |
| Run a shell command that you write | `dtk run -c '...'`, or `... \| dtk filter` | Whatever your wrapper does. |

## Status

| Agent | How it is connected | Automatic | Tried with the real program |
|---|---|---|---|
| Claude Code | `PostToolUse` hook: `dtk init --global` | yes | yes (every release) |
| OpenCode | plugin, `tool.execute.after`: `dtk init --agent opencode` | yes | yes: OpenCode 1.18.29 with a real model (`scripts/opencode-test.sh`: the tool returned 218 tokens, not 2,050, and the model answered right). OpenCode 2.x (the `setup()` entry of the plugin) is written from the documentation and tried only with a stand-in in Node: the real 2.2.6 could not be driven in a clean environment (it gave no events), so it is not tried. |
| Codex CLI | `PostToolUse` hook, the summary is returned as the feedback that replaces the result: `dtk init --agent codex` | yes | no: written from the documentation |
| Gemini CLI | `AfterTool` hook: `dtk init --agent gemini` | yes | no: written from the documentation |
| Copilot CLI | `postToolUse` hook, `modifiedResult`: `dtk init --agent copilot` | yes | no: written from the documentation |
| Cursor | a rule file: `dtk init --agent cursor` | no | not applicable |
| Any other agent | `dtk run`, `dtk filter`, or a rule: `dtk init --agent rules` | by rule | not applicable |

"Written from the documentation" means: the event and the reply have the fields that the
agent's own documentation gives, and tests check them (`tests/harness.rs`, `dtk verify`). It does
not mean that the real program was run. If one of these does not work for you, run
`dtk skeleton` on what happened and open an issue; the template asks for exactly that.

## Why dtk does not rewrite the command

Some agents (Cursor, Gemini CLI, Copilot CLI, Codex, OpenCode) can rewrite a command before it
runs, and the command could be changed to `dtk run -c '...'`. dtk does not do this by itself. The
text of the command is what the agent's allow and deny rules read. A rule such as "deny `rm *`"
would stop matching `dtk run -c 'rm -rf x'`, and an agent in an automatic mode would run it. A hook
that replaces the *result* after the command ran changes no permission, so that is what dtk uses
wherever it can. Cursor's hooks cannot replace the result of a shell command (only of an MCP tool),
so for Cursor dtk gives you a rule, and `dtk run`, and says that this is not automatic.

## Connect an agent

Run the command, then restart the agent so that it reads its configuration. Each command keeps the
hooks that the file already has, makes a backup once, and does nothing when it is run again.
`--uninstall` takes out only dtk's own entry.

```
dtk init --global               Claude Code (as before)
dtk init --agent codex          ~/.codex/hooks.json      (or $CODEX_HOME)
dtk init --agent gemini         ~/.gemini/settings.json
dtk init --agent copilot        ~/.copilot/hooks/dtk.json (or $COPILOT_HOME)
dtk init --agent opencode       ~/.config/opencode/plugins/dtk.ts
dtk init --agent cursor         .cursor/rules/dtk.mdc    (in the folder you are in)
dtk init --agent rules          a block in AGENTS.md     (or --file PATH)
dtk doctor                      says which installed agents have dtk
```

- **Codex** asks you to review a new hook in `/hooks` before it runs, and needs `hooks = true` under
  `[features]` in `config.toml`.
- **OpenCode** finds `dtk` on its own `PATH`. It does not read your shell profile, so a `dtk` that
  only your shell finds is not found by the plugin. Homebrew and the install script put it on the
  `PATH`.
- **rtk** keeps working beside dtk in every agent. rtk rewrites the command before it runs, and
  dtk replaces the result after. dtk reads the full output that rtk saved when it cut one.

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
